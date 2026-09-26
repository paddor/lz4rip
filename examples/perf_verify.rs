//! Local CPU/size regression gate. See DEVELOPMENT.md for calibration.

#[path = "perf_verify/fixtures.rs"]
mod fixtures;

use lz4rip::block::{Compressor, Decompressor, DictCompressor, DictTrainer};
use std::collections::{BTreeMap, BTreeSet};
use std::hint::black_box;
use std::time::{Duration, Instant};

const SIZES: &[usize] = &[64, 128, 256, 512, 1024, 1536, 2048, 4096];
const FAMILIES: &[&str] = &["json", "json-dict", "text", "binary"];
const METRICS: &[&str] = &["encode_ns", "decode_ns", "bytes"];
const POOL: usize = 64;

fn keys() -> BTreeSet<String> {
    FAMILIES
        .iter()
        .flat_map(|family| {
            SIZES.iter().flat_map(move |size| {
                METRICS
                    .iter()
                    .map(move |metric| format!("{family}.{size}.{metric}"))
            })
        })
        .collect()
}

fn thresholds(contents: &str) -> Result<BTreeMap<String, f64>, String> {
    let expected = keys();
    let mut values = BTreeMap::new();
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = line.split_once('=').ok_or("expected key=value")?;
        let key = key.trim();
        if !expected.contains(key) {
            return Err(format!("unknown threshold: {key}"));
        }
        let value: f64 = value.trim().parse().map_err(|_| "invalid threshold")?;
        if !value.is_finite() || value <= 0.0 {
            return Err(format!("threshold must be finite and positive: {key}"));
        }
        if values.insert(key.to_owned(), value).is_some() {
            return Err(format!("duplicate threshold: {key}"));
        }
    }
    for key in expected {
        if !values.contains_key(&key) {
            return Err(format!("missing threshold: {key}"));
        }
    }
    Ok(values)
}

#[cfg(unix)]
fn cpu_ns() -> u64 {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: time points to a writable timespec.
    let rc = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut time) };
    assert_eq!(rc, 0, "thread CPU clock unavailable");
    time.tv_sec as u64 * 1_000_000_000 + time.tv_nsec as u64
}

#[cfg(not(unix))]
fn cpu_ns() -> u64 {
    panic!("performance verification requires a Unix thread CPU clock")
}

#[cfg(target_os = "linux")]
fn pin_cpu() -> Result<(), String> {
    // SAFETY: cpu_set_t is a plain bitmap; all calls receive its actual size.
    unsafe {
        let mut allowed: libc::cpu_set_t = std::mem::zeroed();
        let bytes = std::mem::size_of_val(&allowed);
        if libc::sched_getaffinity(0, bytes, &mut allowed) != 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let cpu = (0..libc::CPU_SETSIZE as usize)
            .find(|&cpu| libc::CPU_ISSET(cpu, &allowed))
            .ok_or("no allowed CPU")?;
        libc::CPU_ZERO(&mut allowed);
        libc::CPU_SET(cpu, &mut allowed);
        if libc::sched_setaffinity(0, bytes, &allowed) != 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        println!("# pinned to CPU {cpu}; use taskset to select another CPU");
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn pin_cpu() -> Result<(), String> {
    Ok(())
}

fn timed_batch(f: &mut impl FnMut(), target_ns: u64) -> f64 {
    let wall = Instant::now();
    let start = cpu_ns();
    let mut calls = 0u64;
    loop {
        for _ in 0..POOL {
            f();
        }
        calls += POOL as u64;
        let elapsed = cpu_ns() - start;
        assert!(
            wall.elapsed() < Duration::from_secs(5),
            "measurement timed out"
        );
        if elapsed >= target_ns {
            return elapsed as f64 / calls as f64;
        }
    }
}

fn measure(mut f: impl FnMut()) -> f64 {
    timed_batch(&mut f, 10_000_000);
    timed_batch(&mut f, 50_000_000)
}

fn record(
    key: &str,
    value: f64,
    samples: &mut BTreeMap<String, Vec<f64>>,
    limits: Option<&BTreeMap<String, f64>>,
) -> Result<(), String> {
    if !value.is_finite() || value <= 0.0 {
        return Err(format!("invalid measurement: {key}={value}"));
    }
    let values = samples.entry(key.to_owned()).or_default();
    values.push(value);
    if values.len() == 3 {
        values.sort_by(f64::total_cmp);
        check(key, values[1], limits)?;
    }
    Ok(())
}

fn check(key: &str, value: f64, limits: Option<&BTreeMap<String, f64>>) -> Result<(), String> {
    if !value.is_finite() || value <= 0.0 {
        return Err(format!("invalid measurement: {key}={value}"));
    }
    if let Some(limits) = limits {
        let limit = limits
            .get(key)
            .ok_or_else(|| format!("missing threshold: {key}"))?;
        println!("{key}={value:.3} # max {limit:.3}");
        if value > *limit {
            return Err(format!(
                "{key}: {value:.3} exceeds {limit:.3} by {:.1}%",
                (value / limit - 1.0) * 100.0
            ));
        }
    } else {
        println!("{key}={value:.3}");
    }
    Ok(())
}

fn run_case(
    family: &str,
    size: usize,
    dict: &[u8],
    samples: &mut BTreeMap<String, Vec<f64>>,
    limits: Option<&BTreeMap<String, f64>>,
) -> Result<(), String> {
    let inputs: Vec<_> = (101..101 + POOL as u32)
        .map(|seed| match family {
            "json" | "json-dict" => fixtures::payload(size, seed),
            "text" => fixtures::text(size, seed),
            "binary" => fixtures::binary(size, seed),
            _ => unreachable!(),
        })
        .collect();
    let with_dict = family == "json-dict";
    let mut plain = Compressor::new();
    let mut dictionary = DictCompressor::new(dict);
    let mut output = vec![0; lz4rip::block::get_maximum_output_size(size)];
    let mut encode = |input: &[u8], out: &mut [u8]| {
        if with_dict {
            dictionary.compress_into(input, out).unwrap()
        } else {
            plain.compress_into(input, out).unwrap()
        }
    };
    let blocks: Vec<_> = inputs
        .iter()
        .map(|input| {
            let len = encode(input, &mut output);
            output[..len].to_vec()
        })
        .collect();
    let decoder = Decompressor::with_dict(if with_dict { dict } else { &[] });
    let mut decoded = vec![0; size];
    for (input, block) in inputs.iter().zip(&blocks) {
        assert_eq!(decoder.decompress_into(block, &mut decoded).unwrap(), size);
        assert_eq!(input, &decoded);
    }
    let prefix = format!("{family}.{size}");
    let bytes = blocks.iter().map(Vec::len).sum::<usize>() as f64 / POOL as f64;
    record(&format!("{prefix}.bytes"), bytes, samples, limits)?;
    let mut index = 0;
    let encode_ns = measure(|| {
        black_box(encode(black_box(&inputs[index]), black_box(&mut output)));
        index = (index + 1) % POOL;
    });
    record(&format!("{prefix}.encode_ns"), encode_ns, samples, limits)?;
    let decode_ns = measure(|| {
        black_box(
            decoder
                .decompress_into(black_box(&blocks[index]), black_box(&mut decoded))
                .unwrap(),
        );
        index = (index + 1) % POOL;
    });
    record(&format!("{prefix}.decode_ns"), decode_ns, samples, limits)
}

fn run() -> Result<(), String> {
    if cfg!(debug_assertions) {
        return Err("run the performance gate with --release".into());
    }
    if cfg!(feature = "paranoid") {
        return Err("performance thresholds target the default codec, without paranoid".into());
    }
    let args: Vec<_> = std::env::args().skip(1).collect();
    let measure_only = match args.as_slice() {
        [] => false,
        [arg] if arg == "--measure-only" => true,
        _ => return Err("usage: perf_verify [--measure-only]".into()),
    };
    let limits = if measure_only {
        None
    } else {
        let contents = std::fs::read_to_string(".perf_hw").map_err(|e| {
            format!("cannot read .perf_hw: {e}; see DEVELOPMENT.md for calibration")
        })?;
        Some(thresholds(&contents)?)
    };
    pin_cpu()?;
    println!("# 64 rotating inputs; three separate passes, 50 ms thread CPU per operation");
    let mut trainer = DictTrainer::new(2048);
    for (size, count) in [
        (64, 2),
        (128, 2),
        (256, 4),
        (512, 8),
        (1024, 8),
        (2048, 4),
        (4096, 4),
    ] {
        for seed in 1..=count {
            trainer.add_sample(&fixtures::payload(size, seed));
        }
    }
    let dict = trainer.train();
    let mut samples = BTreeMap::new();
    for pass in 1..=3 {
        println!("# pass {pass}/3");
        for family in FAMILIES {
            for &size in SIZES {
                run_case(family, size, &dict, &mut samples, limits.as_ref())?;
            }
        }
    }
    println!(
        "# {}: {} limits",
        if measure_only {
            "measurement only"
        } else {
            "PASS"
        },
        keys().len()
    );
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("performance gate FAILED: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn complete_limits() -> String {
        keys()
            .into_iter()
            .map(|key| format!("{key}=1000\n"))
            .collect()
    }

    #[test]
    fn thresholds_fail_closed() {
        let complete = complete_limits();
        assert!(thresholds(&complete).is_ok());
        assert!(thresholds("").is_err());
        assert!(thresholds(&complete.replacen("=1000", "=NaN", 1)).is_err());
        for bad in ["inf", "0", "-1", "typo"] {
            assert!(thresholds(&complete.replacen("=1000", &format!("={bad}"), 1)).is_err());
        }
        assert!(thresholds(&(complete.clone() + "unknown=1\n")).is_err());
        assert!(thresholds(&(complete + "json.1024.encode_ns=1\n")).is_err());
    }

    #[test]
    fn costs_are_independent_gates() {
        let limits = thresholds(&complete_limits()).unwrap();
        for key in [
            "json.1024.encode_ns",
            "json.1024.decode_ns",
            "json.1024.bytes",
        ] {
            assert!(check(key, 1000.0, Some(&limits)).is_ok());
            assert!(check(key, 1001.0, Some(&limits)).is_err());
        }
        assert!(check("missing", 1.0, Some(&limits)).is_err());
        assert!(check("json.1024.encode_ns", f64::NAN, Some(&limits)).is_err());
    }

    #[test]
    fn isolated_spikes_do_not_hide_sustained_regressions() {
        let limits = thresholds(&complete_limits()).unwrap();
        let key = "json.1024.encode_ns";
        for (values, passes) in [
            ([900.0, 2000.0, 900.0], true),
            ([2000.0, 900.0, 2000.0], false),
        ] {
            let mut samples = BTreeMap::new();
            assert!(record(key, values[0], &mut samples, Some(&limits)).is_ok());
            assert!(record(key, values[1], &mut samples, Some(&limits)).is_ok());
            assert_eq!(
                record(key, values[2], &mut samples, Some(&limits)).is_ok(),
                passes
            );
        }
    }

    #[test]
    fn fixtures_rotate_distinct_inputs() {
        for generate in [fixtures::payload, fixtures::text, fixtures::binary] {
            for &size in SIZES {
                let inputs: BTreeSet<_> = (101..165).map(|seed| generate(size, seed)).collect();
                assert_eq!(inputs.len(), POOL);
                assert!(inputs.iter().all(|input| input.len() == size));
            }
        }
    }
}
