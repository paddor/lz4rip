extern crate libc;
extern crate lz4_flex_unsafe;
extern crate lz4_flex_upstream;

use std::io::Write;
use std::os::raw::c_int;
use std::path::PathBuf;
use std::process::Command;

#[repr(C)]
struct LZ4Stream {
    _opaque: [u8; 0],
}

unsafe extern "C" {
    fn LZ4_createStream() -> *mut LZ4Stream;
    fn LZ4_freeStream(stream: *mut LZ4Stream) -> c_int;
    fn LZ4_resetStream_fast(stream: *mut LZ4Stream);
    fn LZ4_loadDict(stream: *mut LZ4Stream, dict: *const u8, dict_size: c_int) -> c_int;
    fn LZ4_compress_fast_continue(
        stream: *mut LZ4Stream,
        src: *const u8,
        dst: *mut u8,
        src_size: c_int,
        dst_capacity: c_int,
        acceleration: c_int,
    ) -> c_int;
    fn LZ4_decompress_safe_usingDict(
        src: *const u8,
        dst: *mut u8,
        compressed_size: c_int,
        dst_capacity: c_int,
        dict: *const u8,
        dict_size: c_int,
    ) -> c_int;
}

fn cpu_nanos() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut ts) };
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

#[derive(Clone)]
struct BenchResult {
    codec: String,
    input_name: String,
    input_size: usize,
    compressed_size: usize,
    compress_ns: f64,
    decompress_ns: f64,
}

impl BenchResult {
    fn to_json(&self) -> String {
        format!(
            r#"{{"codec": "{}", "input": "{}", "input_size": {}, "compressed_size": {}, "compress_ns": {:.1}, "decompress_ns": {:.1}}}"#,
            self.codec,
            self.input_name,
            self.input_size,
            self.compressed_size,
            self.compress_ns,
            self.decompress_ns
        )
    }

    fn from_json(line: &str) -> Option<Self> {
        let line = line.trim().trim_matches(',');
        if line == "[" || line == "]" || line.is_empty() {
            return None;
        }
        let get = |key: &str| -> Option<String> {
            let prefix = format!("\"{key}\": ");
            let start = line.find(&prefix)? + prefix.len();
            let rest = &line[start..];
            if let Some(stripped) = rest.strip_prefix('"') {
                let end = stripped.find('"')?;
                Some(stripped[..end].to_string())
            } else {
                let end = rest.find([',', '}']).unwrap_or(rest.len());
                Some(rest[..end].to_string())
            }
        };
        Some(BenchResult {
            codec: get("codec")?,
            input_name: get("input")?,
            input_size: get("input_size")?.parse().ok()?,
            compressed_size: get("compressed_size")?.parse().ok()?,
            compress_ns: get("compress_ns")?.parse().ok()?,
            decompress_ns: get("decompress_ns")?.parse().ok()?,
        })
    }
}

fn bench_loop<F: FnMut()>(warmup: usize, target_ns: u64, rounds: usize, mut f: F) -> f64 {
    for _ in 0..warmup {
        f();
    }
    let mut best = f64::MAX;
    for _ in 0..rounds {
        let mut iters = 0u64;
        let start = cpu_nanos();
        loop {
            std::hint::black_box(&mut f)();
            iters += 1;
            if cpu_nanos() - start >= target_ns {
                break;
            }
        }
        let elapsed = cpu_nanos() - start;
        let ns_per_op = elapsed as f64 / iters as f64;
        if ns_per_op < best {
            best = ns_per_op;
        }
    }
    best
}

fn bench_lz4rip(data: &[u8], name: &str, target_ns: u64) -> BenchResult {
    let max_out = lz4rip::block::get_maximum_output_size(data.len());
    let mut comp_buf = vec![0u8; max_out];
    let comp_len = lz4rip::block::compress_into(data, &mut comp_buf).unwrap();
    let compressed = comp_buf[..comp_len].to_vec();
    let mut decomp_buf = vec![0u8; data.len()];

    let compress_ns = bench_loop(3, target_ns, 10, || {
        let _ = lz4rip::block::compress_into(
            std::hint::black_box(data),
            std::hint::black_box(&mut comp_buf),
        );
    });

    let decompress_ns = bench_loop(3, target_ns, 10, || {
        let _ = lz4rip::block::decompress_into(
            std::hint::black_box(&compressed),
            std::hint::black_box(&mut decomp_buf),
        );
    });

    BenchResult {
        codec: LZ4RIP_CODEC.to_string(),
        input_name: name.to_string(),
        input_size: data.len(),
        compressed_size: comp_len,
        compress_ns,
        decompress_ns,
    }
}

fn bench_lz4_flex_upstream(data: &[u8], name: &str, target_ns: u64) -> BenchResult {
    let max_out = lz4_flex_upstream::block::get_maximum_output_size(data.len());
    let mut comp_buf = vec![0u8; max_out];
    let comp_len = lz4_flex_upstream::block::compress_into(data, &mut comp_buf).unwrap();
    let compressed = comp_buf[..comp_len].to_vec();
    let mut decomp_buf = vec![0u8; data.len()];

    let compress_ns = bench_loop(3, target_ns, 10, || {
        let _ = lz4_flex_upstream::block::compress_into(
            std::hint::black_box(data),
            std::hint::black_box(&mut comp_buf),
        );
    });

    let decompress_ns = bench_loop(3, target_ns, 10, || {
        let _ = lz4_flex_upstream::block::decompress_into(
            std::hint::black_box(&compressed),
            std::hint::black_box(&mut decomp_buf),
        );
    });

    BenchResult {
        codec: "lz4_flex".to_string(),
        input_name: name.to_string(),
        input_size: data.len(),
        compressed_size: comp_len,
        compress_ns,
        decompress_ns,
    }
}

fn bench_lz4_flex_unsafe(data: &[u8], name: &str, target_ns: u64) -> BenchResult {
    let max_out = lz4_flex_unsafe::block::get_maximum_output_size(data.len());
    let mut comp_buf = vec![0u8; max_out];
    let comp_len = lz4_flex_unsafe::block::compress_into(data, &mut comp_buf).unwrap();
    let compressed = comp_buf[..comp_len].to_vec();
    let mut decomp_buf = vec![0u8; data.len()];

    let compress_ns = bench_loop(3, target_ns, 10, || {
        let _ = lz4_flex_unsafe::block::compress_into(
            std::hint::black_box(data),
            std::hint::black_box(&mut comp_buf),
        );
    });

    let decompress_ns = bench_loop(3, target_ns, 10, || {
        let _ = lz4_flex_unsafe::block::decompress_into(
            std::hint::black_box(&compressed),
            std::hint::black_box(&mut decomp_buf),
        );
    });

    BenchResult {
        codec: "lz4_flex unsafe".to_string(),
        input_name: name.to_string(),
        input_size: data.len(),
        compressed_size: comp_len,
        compress_ns,
        decompress_ns,
    }
}

fn bench_c_lz4(data: &[u8], name: &str, target_ns: u64) -> BenchResult {
    let max_out = lzzzz::lz4::max_compressed_size(data.len());
    let mut comp_buf = vec![0u8; max_out];
    let comp_len =
        lzzzz::lz4::compress(data, &mut comp_buf, lzzzz::lz4::ACC_LEVEL_DEFAULT).unwrap();
    let compressed = comp_buf[..comp_len].to_vec();
    let mut decomp_buf = vec![0u8; data.len()];

    let compress_ns = bench_loop(3, target_ns, 10, || {
        let _ = lzzzz::lz4::compress(
            std::hint::black_box(data),
            std::hint::black_box(&mut comp_buf),
            lzzzz::lz4::ACC_LEVEL_DEFAULT,
        );
    });

    let decompress_ns = bench_loop(3, target_ns, 10, || {
        let _ = lzzzz::lz4::decompress(
            std::hint::black_box(&compressed),
            std::hint::black_box(&mut decomp_buf),
        );
    });

    BenchResult {
        codec: "C lz4".to_string(),
        input_name: name.to_string(),
        input_size: data.len(),
        compressed_size: comp_len,
        compress_ns,
        decompress_ns,
    }
}

fn bench_lz4rip_dict(data: &[u8], dict: &[u8], name: &str, target_ns: u64) -> BenchResult {
    let mut compressor = lz4rip::block::DictCompressor::new(dict);
    let max_out = lz4rip::block::get_maximum_output_size(data.len());
    let mut comp_buf = vec![0u8; max_out];
    let comp_len = compressor.compress_into(data, &mut comp_buf).unwrap();
    let compressed = comp_buf[..comp_len].to_vec();

    let decompressor = lz4rip::block::Decompressor::with_dict(dict);
    let mut decomp_buf = vec![0u8; data.len()];
    let check = decompressor
        .decompress_into(&compressed, &mut decomp_buf)
        .unwrap();
    assert_eq!(check, data.len());
    assert_eq!(&decomp_buf[..], data);

    let compress_ns = bench_loop(3, target_ns, 10, || {
        let _ = std::hint::black_box(&mut compressor).compress_into(
            std::hint::black_box(data),
            std::hint::black_box(&mut comp_buf),
        );
    });

    let decompress_ns = bench_loop(3, target_ns, 10, || {
        let _ = decompressor.decompress_into(
            std::hint::black_box(&compressed),
            std::hint::black_box(&mut decomp_buf),
        );
    });

    BenchResult {
        codec: LZ4RIP_DICT_CODEC.to_string(),
        input_name: name.to_string(),
        input_size: data.len(),
        compressed_size: comp_len,
        compress_ns,
        decompress_ns,
    }
}

fn bench_c_lz4_dict(data: &[u8], dict: &[u8], name: &str, target_ns: u64) -> BenchResult {
    let max_out = lzzzz::lz4::max_compressed_size(data.len());
    let mut comp_buf = vec![0u8; max_out];
    let mut decomp_buf = vec![0u8; data.len()];

    let stream = unsafe { LZ4_createStream() };
    assert!(!stream.is_null());

    // initial compress to get compressed_size
    unsafe {
        LZ4_resetStream_fast(stream);
        LZ4_loadDict(stream, dict.as_ptr(), dict.len() as c_int);
    }
    let comp_len = unsafe {
        LZ4_compress_fast_continue(
            stream,
            data.as_ptr(),
            comp_buf.as_mut_ptr(),
            data.len() as c_int,
            max_out as c_int,
            1,
        )
    };
    assert!(comp_len > 0, "C lz4 dict compress failed");
    let comp_len = comp_len as usize;
    let compressed = comp_buf[..comp_len].to_vec();

    // verify roundtrip
    let dec_len = unsafe {
        LZ4_decompress_safe_usingDict(
            compressed.as_ptr(),
            decomp_buf.as_mut_ptr(),
            compressed.len() as c_int,
            decomp_buf.len() as c_int,
            dict.as_ptr(),
            dict.len() as c_int,
        )
    };
    assert_eq!(dec_len as usize, data.len());
    assert_eq!(&decomp_buf[..], data);

    let compress_ns = bench_loop(3, target_ns, 10, || unsafe {
        LZ4_resetStream_fast(stream);
        LZ4_loadDict(stream, dict.as_ptr(), dict.len() as c_int);
        LZ4_compress_fast_continue(
            stream,
            data.as_ptr(),
            comp_buf.as_mut_ptr(),
            data.len() as c_int,
            max_out as c_int,
            1,
        );
    });

    let decompress_ns = bench_loop(3, target_ns, 10, || unsafe {
        LZ4_decompress_safe_usingDict(
            compressed.as_ptr(),
            decomp_buf.as_mut_ptr(),
            compressed.len() as c_int,
            decomp_buf.len() as c_int,
            dict.as_ptr(),
            dict.len() as c_int,
        );
    });

    unsafe { LZ4_freeStream(stream) };

    BenchResult {
        codec: "C lz4 (dict 2K)".to_string(),
        input_name: name.to_string(),
        input_size: data.len(),
        compressed_size: comp_len,
        compress_ns,
        decompress_ns,
    }
}

fn arch() -> &'static str {
    std::env::consts::ARCH
}

fn cache_dir_for(subdir: &str) -> PathBuf {
    let dir = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
        .join(".cache")
        .join("lz4rip")
        .join(arch())
        .join(subdir);
    std::fs::create_dir_all(&dir).ok();
    dir
}

fn cache_dir() -> PathBuf {
    cache_dir_for("")
}

fn codec_cache_path(codec: &str) -> PathBuf {
    let filename = codec.replace(' ', "_").replace(['(', ')'], "");
    cache_dir().join(format!("{filename}.jsonl"))
}

fn codec_cache_path_in(subdir: &str, codec: &str) -> PathBuf {
    let filename = codec.replace(' ', "_").replace(['(', ')'], "");
    cache_dir_for(subdir).join(format!("{filename}.jsonl"))
}

fn load_cache(codecs: &[&str]) -> Vec<BenchResult> {
    let mut results = Vec::new();
    for codec in codecs {
        let path = codec_cache_path(codec);
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        results.extend(content.lines().filter_map(BenchResult::from_json));
    }
    results
}

fn save_cache(results: &[BenchResult], codecs: &[&str]) {
    for codec in codecs {
        let entries: Vec<_> = results.iter().filter(|r| r.codec == *codec).collect();
        if entries.is_empty() {
            continue;
        }
        let path = codec_cache_path(codec);
        let mut f = std::fs::File::create(&path).unwrap();
        for r in &entries {
            writeln!(f, "{}", r.to_json()).unwrap();
        }
        eprintln!("cached {} results to {}", entries.len(), path.display());
    }
}

fn save_results_to(subdir: &str, results: &[BenchResult]) {
    let mut by_codec: std::collections::HashMap<&str, Vec<&BenchResult>> =
        std::collections::HashMap::new();
    for r in results {
        by_codec.entry(r.codec.as_str()).or_default().push(r);
    }
    for (codec, entries) in &by_codec {
        let path = codec_cache_path_in(subdir, codec);
        let mut f = std::fs::File::create(&path).unwrap();
        for r in entries {
            writeln!(f, "{}", r.to_json()).unwrap();
        }
        eprintln!("cached {} results to {}", entries.len(), path.display());
    }
}

// The lz4rip codec is labeled per build: a `--features paranoid` build of this
// example exercises the pure-safe code path and reports under the "paranoid"
// names so its results cache separately and appear as their own chart bars.
#[cfg(not(feature = "paranoid"))]
const LZ4RIP_CODEC: &str = "lz4rip";
#[cfg(feature = "paranoid")]
const LZ4RIP_CODEC: &str = "lz4rip paranoid";
#[cfg(not(feature = "paranoid"))]
const LZ4RIP_DICT_CODEC: &str = "lz4rip (dict 2K)";
#[cfg(feature = "paranoid")]
const LZ4RIP_DICT_CODEC: &str = "lz4rip paranoid (dict 2K)";

#[cfg(not(feature = "paranoid"))]
const CODECS: &[&str] = &["C lz4", "lz4rip", "lz4_flex", "lz4_flex unsafe"];
#[cfg(feature = "paranoid")]
const CODECS: &[&str] = &["C lz4", "lz4rip paranoid", "lz4_flex", "lz4_flex unsafe"];
#[cfg(not(feature = "paranoid"))]
const DICT_CODECS: &[&str] = &["C lz4 (dict 2K)", "lz4rip (dict 2K)"];
#[cfg(feature = "paranoid")]
const DICT_CODECS: &[&str] = &["C lz4 (dict 2K)", "lz4rip paranoid (dict 2K)"];

const SILESIA_DOWNLOADS: &[(&str, &str)] = &[
    (
        "corpus/silesia/dickens",
        "https://sun.aei.polsl.pl/~sdeor/corpus/dickens.bz2",
    ),
    (
        "corpus/silesia/mozilla",
        "https://sun.aei.polsl.pl/~sdeor/corpus/mozilla.bz2",
    ),
    (
        "corpus/silesia/mr",
        "https://sun.aei.polsl.pl/~sdeor/corpus/mr.bz2",
    ),
    (
        "corpus/silesia/nci",
        "https://sun.aei.polsl.pl/~sdeor/corpus/nci.bz2",
    ),
    (
        "corpus/silesia/ooffice",
        "https://sun.aei.polsl.pl/~sdeor/corpus/ooffice.bz2",
    ),
    (
        "corpus/silesia/osdb",
        "https://sun.aei.polsl.pl/~sdeor/corpus/osdb.bz2",
    ),
    (
        "corpus/silesia/reymont",
        "https://sun.aei.polsl.pl/~sdeor/corpus/reymont.bz2",
    ),
    (
        "corpus/silesia/samba",
        "https://sun.aei.polsl.pl/~sdeor/corpus/samba.bz2",
    ),
    (
        "corpus/silesia/sao",
        "https://sun.aei.polsl.pl/~sdeor/corpus/sao.bz2",
    ),
    (
        "corpus/silesia/webster",
        "https://sun.aei.polsl.pl/~sdeor/corpus/webster.bz2",
    ),
    (
        "corpus/silesia/x-ray",
        "https://sun.aei.polsl.pl/~sdeor/corpus/x-ray.bz2",
    ),
    (
        "corpus/silesia/xml",
        "https://sun.aei.polsl.pl/~sdeor/corpus/xml.bz2",
    ),
];

fn ensure_corpus() {
    for &(path, url) in SILESIA_DOWNLOADS {
        if std::fs::metadata(path).is_ok() {
            continue;
        }
        eprintln!("downloading {url} ...");
        let dir = PathBuf::from(path).parent().unwrap().to_owned();
        std::fs::create_dir_all(&dir).ok();
        let status = Command::new("sh")
            .arg("-c")
            .arg(format!("curl -fSL '{url}' | bzip2 -d > '{path}'"))
            .status();
        match status {
            Ok(s) if s.success() => {
                let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
                eprintln!("  saved {path} ({size} bytes)");
            }
            _ => {
                eprintln!("  failed to download {path}, skipping");
                std::fs::remove_file(path).ok();
            }
        }
    }
}

const ALL_FILES: &[&str] = &[
    "corpus/silesia/dickens",
    "corpus/silesia/mozilla",
    "corpus/silesia/mr",
    "corpus/silesia/nci",
    "corpus/silesia/ooffice",
    "corpus/silesia/osdb",
    "corpus/silesia/reymont",
    "corpus/silesia/samba",
    "corpus/silesia/sao",
    "corpus/silesia/webster",
    "corpus/silesia/x-ray",
    "corpus/silesia/xml",
];

fn load_silesia_inputs() -> Vec<(String, Vec<u8>)> {
    let mut inputs = Vec::new();
    for &(path, _) in SILESIA_DOWNLOADS {
        match std::fs::read(path) {
            Ok(data) => {
                let name = path.rsplit('/').next().unwrap().to_string();
                inputs.push((name, data));
            }
            Err(e) => eprintln!("skipping {path}: {e}"),
        }
    }
    inputs
}

fn train_silesia_dict_from_inputs(inputs: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut trainer = lz4rip::block::DictTrainer::new(2048);
    for (_, data) in inputs {
        let sample_len = 1024.min(data.len());
        if sample_len < 4 {
            continue;
        }
        let start = (data.len() - sample_len) / 2;
        trainer.add_sample(&data[start..start + sample_len]);
    }
    let dict = trainer.train();
    eprintln!("trained Silesia dict: {} bytes", dict.len());
    dict
}

fn prefixed_input_name(name: &str, size: usize) -> String {
    format!("{name}_{size}")
}

const SWEEP_SIZES: &[usize] = &[
    64, 128, 256, 512, 1024, 2048, 4096, 8192, 16384, 32768, 65536, 131072, 262144, 524288, 1048576,
];

fn run_sweep(dict: &[u8], inputs: &[(String, Vec<u8>)]) {
    let target_ns = 20_000_000u64;
    let mut compressor = lz4rip::block::DictCompressor::new(dict);
    let decompressor = lz4rip::block::Decompressor::with_dict(dict);

    let stream = unsafe { LZ4_createStream() };
    assert!(!stream.is_null());

    let mut all_results: Vec<BenchResult> = Vec::new();

    for &size in SWEEP_SIZES {
        for (source_name, source) in inputs {
            if source.len() < size {
                continue;
            }
            let data = source[..size].to_vec();
            let name = prefixed_input_name(source_name, size);

            let max_out = lz4rip::block::get_maximum_output_size(data.len());
            let mut comp_buf = vec![0u8; max_out];
            let mut decomp_buf = vec![0u8; data.len()];

            // lz4rip (no dict)
            {
                let comp_len = lz4rip::block::compress_into(&data, &mut comp_buf).unwrap();
                let compressed = comp_buf[..comp_len].to_vec();
                let compress_ns = bench_loop(3, target_ns, 10, || {
                    let _ = lz4rip::block::compress_into(
                        std::hint::black_box(&data),
                        std::hint::black_box(&mut comp_buf),
                    );
                });
                let decompress_ns = bench_loop(3, target_ns, 10, || {
                    let _ = lz4rip::block::decompress_into(
                        std::hint::black_box(&compressed),
                        std::hint::black_box(&mut decomp_buf),
                    );
                });
                let r = BenchResult {
                    codec: "lz4rip".into(),
                    input_name: name.clone(),
                    input_size: data.len(),
                    compressed_size: comp_len,
                    compress_ns,
                    decompress_ns,
                };
                eprintln!(
                    "  lz4rip x {name}: {compress_ns:.0} ns comp, {decompress_ns:.0} ns decomp"
                );
                all_results.push(r);
            }

            // lz4rip (dict)
            {
                let comp_len = compressor.compress_into(&data, &mut comp_buf).unwrap();
                let compressed = comp_buf[..comp_len].to_vec();
                let compress_ns = bench_loop(3, target_ns, 10, || {
                    let _ = std::hint::black_box(&mut compressor).compress_into(
                        std::hint::black_box(&data),
                        std::hint::black_box(&mut comp_buf),
                    );
                });
                let decompress_ns = bench_loop(3, target_ns, 10, || {
                    let _ = decompressor.decompress_into(
                        std::hint::black_box(&compressed),
                        std::hint::black_box(&mut decomp_buf),
                    );
                });
                let r = BenchResult {
                    codec: "lz4rip (dict)".into(),
                    input_name: name.clone(),
                    input_size: data.len(),
                    compressed_size: comp_len,
                    compress_ns,
                    decompress_ns,
                };
                eprintln!(
                    "  lz4rip (dict) x {name}: {compress_ns:.0} ns comp, {decompress_ns:.0} ns decomp"
                );
                all_results.push(r);
            }

            // C lz4 (no dict)
            {
                let comp_len =
                    lzzzz::lz4::compress(&data, &mut comp_buf, lzzzz::lz4::ACC_LEVEL_DEFAULT)
                        .unwrap();
                let compressed = comp_buf[..comp_len].to_vec();
                let compress_ns = bench_loop(3, target_ns, 10, || {
                    let _ = lzzzz::lz4::compress(
                        std::hint::black_box(&data),
                        std::hint::black_box(&mut comp_buf),
                        lzzzz::lz4::ACC_LEVEL_DEFAULT,
                    );
                });
                let decompress_ns = bench_loop(3, target_ns, 10, || {
                    let _ = lzzzz::lz4::decompress(
                        std::hint::black_box(&compressed),
                        std::hint::black_box(&mut decomp_buf),
                    );
                });
                let r = BenchResult {
                    codec: "C lz4".into(),
                    input_name: name.clone(),
                    input_size: data.len(),
                    compressed_size: comp_len,
                    compress_ns,
                    decompress_ns,
                };
                eprintln!(
                    "  C lz4 x {name}: {compress_ns:.0} ns comp, {decompress_ns:.0} ns decomp"
                );
                all_results.push(r);
            }

            // C lz4 (dict)
            {
                unsafe {
                    LZ4_resetStream_fast(stream);
                    LZ4_loadDict(stream, dict.as_ptr(), dict.len() as c_int);
                }
                let comp_len = unsafe {
                    LZ4_compress_fast_continue(
                        stream,
                        data.as_ptr(),
                        comp_buf.as_mut_ptr(),
                        data.len() as c_int,
                        max_out as c_int,
                        1,
                    )
                } as usize;
                let compressed = comp_buf[..comp_len].to_vec();
                let compress_ns = bench_loop(3, target_ns, 10, || unsafe {
                    LZ4_resetStream_fast(stream);
                    LZ4_loadDict(stream, dict.as_ptr(), dict.len() as c_int);
                    LZ4_compress_fast_continue(
                        stream,
                        data.as_ptr(),
                        comp_buf.as_mut_ptr(),
                        data.len() as c_int,
                        max_out as c_int,
                        1,
                    );
                });
                let decompress_ns = bench_loop(3, target_ns, 10, || unsafe {
                    LZ4_decompress_safe_usingDict(
                        compressed.as_ptr(),
                        decomp_buf.as_mut_ptr(),
                        compressed.len() as c_int,
                        decomp_buf.len() as c_int,
                        dict.as_ptr(),
                        dict.len() as c_int,
                    );
                });
                let r = BenchResult {
                    codec: "C lz4 (dict)".into(),
                    input_name: name.clone(),
                    input_size: data.len(),
                    compressed_size: comp_len,
                    compress_ns,
                    decompress_ns,
                };
                eprintln!(
                    "  C lz4 (dict) x {name}: {compress_ns:.0} ns comp, {decompress_ns:.0} ns decomp"
                );
                all_results.push(r);
            }
        }
    }

    unsafe { LZ4_freeStream(stream) };
    save_results_to("sweep", &all_results);
}

const STRUCTURED_SIZES: &[usize] = &[256, 512, 1024, 2048, 4096, 8192];
const STRUCTURED_CODECS: &[&str] = &["C lz4", "lz4rip", "lz4_flex unsafe", "lz4_flex"];

fn run_structured(only: &[String], inputs: &[(String, Vec<u8>)]) {
    let target_ns = 20_000_000u64;
    let mut all_results: Vec<BenchResult> = Vec::new();

    for (source_name, source) in inputs {
        for &size in STRUCTURED_SIZES {
            if source.len() < size {
                continue;
            }
            let data = source[..size].to_vec();
            let name = prefixed_input_name(source_name, size);
            let max_out = lz4rip::block::get_maximum_output_size(data.len());

            for &codec in STRUCTURED_CODECS {
                if !only.is_empty() && !only.iter().any(|o| codec.contains(o.as_str())) {
                    continue;
                }

                eprintln!("  {codec} x {name}: benchmarking...");

                let r = match codec {
                    "C lz4" => {
                        // C lz4 stream API: LZ4_resetStream_fast reuses table for <4KB
                        let stream = unsafe { LZ4_createStream() };
                        assert!(!stream.is_null());
                        let mut comp_buf = vec![0u8; max_out];

                        unsafe {
                            LZ4_resetStream_fast(stream);
                        }
                        let comp_len = unsafe {
                            LZ4_compress_fast_continue(
                                stream,
                                data.as_ptr(),
                                comp_buf.as_mut_ptr(),
                                data.len() as c_int,
                                max_out as c_int,
                                1,
                            )
                        };
                        assert!(comp_len > 0);
                        let comp_len = comp_len as usize;
                        let compressed = comp_buf[..comp_len].to_vec();
                        let mut decomp_buf = vec![0u8; data.len()];

                        let compress_ns = bench_loop(3, target_ns, 10, || unsafe {
                            LZ4_resetStream_fast(stream);
                            LZ4_compress_fast_continue(
                                stream,
                                data.as_ptr(),
                                comp_buf.as_mut_ptr(),
                                data.len() as c_int,
                                max_out as c_int,
                                1,
                            );
                        });

                        let decompress_ns = bench_loop(3, target_ns, 10, || {
                            let _ = lzzzz::lz4::decompress(
                                std::hint::black_box(&compressed),
                                std::hint::black_box(&mut decomp_buf),
                            );
                        });

                        unsafe { LZ4_freeStream(stream) };

                        BenchResult {
                            codec: "C lz4".to_string(),
                            input_name: name.clone(),
                            input_size: data.len(),
                            compressed_size: comp_len,
                            compress_ns,
                            decompress_ns,
                        }
                    }
                    "lz4rip" => {
                        // Compressor reuse: epoch trick skips memset for <=8KB
                        let mut compressor = lz4rip::block::Compressor::new();
                        let mut comp_buf = vec![0u8; max_out];
                        let comp_len = compressor.compress_into(&data, &mut comp_buf).unwrap();
                        let compressed = comp_buf[..comp_len].to_vec();
                        let mut decomp_buf = vec![0u8; data.len()];

                        let compress_ns = bench_loop(3, target_ns, 10, || {
                            let _ = std::hint::black_box(&mut compressor).compress_into(
                                std::hint::black_box(&data),
                                std::hint::black_box(&mut comp_buf),
                            );
                        });

                        let decompress_ns = bench_loop(3, target_ns, 10, || {
                            let _ = lz4rip::block::decompress_into(
                                std::hint::black_box(&compressed),
                                std::hint::black_box(&mut decomp_buf),
                            );
                        });

                        BenchResult {
                            codec: "lz4rip".to_string(),
                            input_name: name.clone(),
                            input_size: data.len(),
                            compressed_size: comp_len,
                            compress_ns,
                            decompress_ns,
                        }
                    }
                    "lz4_flex unsafe" => bench_lz4_flex_unsafe(&data, &name, target_ns),
                    "lz4_flex" => bench_lz4_flex_upstream(&data, &name, target_ns),
                    _ => unreachable!(),
                };

                all_results.push(r);
            }
        }
    }

    save_results_to("structured", &all_results);
}

fn run_structured_dict(only: &[String], inputs: &[(String, Vec<u8>)], dict: &[u8]) {
    let target_ns = 20_000_000u64;
    let dict_codecs: &[&str] = &["C lz4 (dict 2K)", "lz4rip (dict 2K)"];

    let mut compressor = lz4rip::block::DictCompressor::new(dict);
    let decompressor = lz4rip::block::Decompressor::with_dict(dict);

    let stream = unsafe { LZ4_createStream() };
    assert!(!stream.is_null());

    let mut all_results: Vec<BenchResult> = Vec::new();

    for (source_name, source) in inputs {
        for &size in STRUCTURED_SIZES {
            if source.len() < size {
                continue;
            }
            let data = source[..size].to_vec();
            let name = prefixed_input_name(source_name, size);
            let max_out = lz4rip::block::get_maximum_output_size(data.len());

            for &codec in dict_codecs {
                if !only.is_empty() && !only.iter().any(|o| codec.contains(o.as_str())) {
                    continue;
                }

                eprintln!("  {codec} x {name}: benchmarking...");

                let r = match codec {
                    "C lz4 (dict 2K)" => {
                        let mut comp_buf = vec![0u8; max_out];
                        unsafe {
                            LZ4_resetStream_fast(stream);
                            LZ4_loadDict(stream, dict.as_ptr(), dict.len() as c_int);
                        }
                        let comp_len = unsafe {
                            LZ4_compress_fast_continue(
                                stream,
                                data.as_ptr(),
                                comp_buf.as_mut_ptr(),
                                data.len() as c_int,
                                max_out as c_int,
                                1,
                            )
                        };
                        assert!(comp_len > 0);
                        let comp_len = comp_len as usize;
                        let compressed = comp_buf[..comp_len].to_vec();
                        let mut decomp_buf = vec![0u8; data.len()];

                        let compress_ns = bench_loop(3, target_ns, 10, || unsafe {
                            LZ4_resetStream_fast(stream);
                            LZ4_loadDict(stream, dict.as_ptr(), dict.len() as c_int);
                            LZ4_compress_fast_continue(
                                stream,
                                data.as_ptr(),
                                comp_buf.as_mut_ptr(),
                                data.len() as c_int,
                                max_out as c_int,
                                1,
                            );
                        });

                        let decompress_ns = bench_loop(3, target_ns, 10, || unsafe {
                            LZ4_decompress_safe_usingDict(
                                compressed.as_ptr(),
                                decomp_buf.as_mut_ptr(),
                                compressed.len() as c_int,
                                decomp_buf.len() as c_int,
                                dict.as_ptr(),
                                dict.len() as c_int,
                            );
                        });

                        BenchResult {
                            codec: "C lz4 (dict 2K)".to_string(),
                            input_name: name.clone(),
                            input_size: data.len(),
                            compressed_size: comp_len,
                            compress_ns,
                            decompress_ns,
                        }
                    }
                    "lz4rip (dict 2K)" => {
                        let mut comp_buf = vec![0u8; max_out];
                        let comp_len = compressor.compress_into(&data, &mut comp_buf).unwrap();
                        let compressed = comp_buf[..comp_len].to_vec();
                        let mut decomp_buf = vec![0u8; data.len()];

                        let compress_ns = bench_loop(3, target_ns, 10, || {
                            let _ = std::hint::black_box(&mut compressor).compress_into(
                                std::hint::black_box(&data),
                                std::hint::black_box(&mut comp_buf),
                            );
                        });

                        let decompress_ns = bench_loop(3, target_ns, 10, || {
                            let _ = decompressor.decompress_into(
                                std::hint::black_box(&compressed),
                                std::hint::black_box(&mut decomp_buf),
                            );
                        });

                        BenchResult {
                            codec: "lz4rip (dict 2K)".to_string(),
                            input_name: name.clone(),
                            input_size: data.len(),
                            compressed_size: comp_len,
                            compress_ns,
                            decompress_ns,
                        }
                    }
                    _ => unreachable!(),
                };

                all_results.push(r);
            }
        }
    }

    unsafe { LZ4_freeStream(stream) };
    save_results_to("structured", &all_results);
}

fn main() {
    ensure_corpus();

    let args: Vec<String> = std::env::args().collect();
    let mut only: Vec<String> = Vec::new();
    let mut dict_path: Option<String> = None;
    let mut dict_silesia = false;
    let mut sweep_dict: Option<String> = None;
    let mut structured = false;
    let mut structured_dict = false;
    let mut file_filter: Vec<String> = Vec::new();
    let mut extra_files: Vec<String> = Vec::new();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--impl" => {
                i += 1;
                if i < args.len() {
                    only.push(args[i].clone());
                }
            }
            "--dict" => {
                i += 1;
                if i < args.len() {
                    dict_path = Some(args[i].clone());
                }
            }
            "--dict-silesia" => {
                dict_silesia = true;
            }
            "--sweep" => {
                if i + 1 < args.len() && !args[i + 1].starts_with('-') {
                    i += 1;
                    sweep_dict = Some(args[i].clone());
                } else {
                    sweep_dict = Some(String::new());
                }
            }
            "--structured" => {
                structured = true;
            }
            "--structured-dict" => {
                structured_dict = true;
            }
            "--files" => {
                i += 1;
                if i < args.len() {
                    file_filter.extend(args[i].split(',').map(|s| s.to_string()));
                }
            }
            "--extra" => {
                i += 1;
                if i < args.len() {
                    extra_files.push(args[i].clone());
                }
            }
            _ => {}
        }
        i += 1;
    }

    if structured {
        let inputs = load_silesia_inputs();
        run_structured(&only, &inputs);
        return;
    }

    if structured_dict {
        let inputs = load_silesia_inputs();
        let dict = train_silesia_dict_from_inputs(&inputs);
        run_structured_dict(&only, &inputs, &dict);
        return;
    }

    if sweep_dict.is_some() {
        let inputs = load_silesia_inputs();
        let dict = match sweep_dict {
            Some(ref dp) if !dp.is_empty() => {
                std::fs::read(dp).unwrap_or_else(|e| panic!("cannot read dict {dp}: {e}"))
            }
            _ => {
                eprintln!("  training sweep dict (2048 bytes)...");
                train_silesia_dict_from_inputs(&inputs)
            }
        };
        run_sweep(&dict, &inputs);
        return;
    }

    let dict_data = if dict_silesia {
        let inputs = load_silesia_inputs();
        Some(train_silesia_dict_from_inputs(&inputs))
    } else {
        dict_path.map(|p| std::fs::read(&p).unwrap_or_else(|e| panic!("cannot read dict {p}: {e}")))
    };

    let codecs: &[&str] = if dict_data.is_some() {
        DICT_CODECS
    } else {
        CODECS
    };
    let target_ns = 20_000_000u64;
    let cached = load_cache(codecs);
    let mut results: Vec<BenchResult> = Vec::new();

    let all_paths: Vec<&str> = ALL_FILES
        .iter()
        .copied()
        .chain(extra_files.iter().map(|s| s.as_str()))
        .collect();

    for path in &all_paths {
        let name = path.rsplit('/').next().unwrap();
        if !file_filter.is_empty() && !file_filter.iter().any(|f| f == name) {
            continue;
        }

        let data = match std::fs::read(path) {
            Ok(d) => d,
            Err(_) => {
                eprintln!("skipping {path}: not found");
                continue;
            }
        };

        for &codec in codecs {
            let should_run = only.is_empty() || only.iter().any(|o| codec.contains(o.as_str()));

            if !should_run {
                if let Some(c) = cached
                    .iter()
                    .find(|c| c.codec == codec && c.input_name == name)
                {
                    eprintln!("  {codec} x {name}: cached");
                    results.push(c.clone());
                    continue;
                }
            }

            eprintln!("  {codec} x {name}: benchmarking...");
            let r = if codec == LZ4RIP_CODEC {
                bench_lz4rip(&data, name, target_ns)
            } else if codec == LZ4RIP_DICT_CODEC {
                bench_lz4rip_dict(&data, dict_data.as_ref().unwrap(), name, target_ns)
            } else {
                match codec {
                    "C lz4" => bench_c_lz4(&data, name, target_ns),
                    "lz4_flex" => bench_lz4_flex_upstream(&data, name, target_ns),
                    "lz4_flex unsafe" => bench_lz4_flex_unsafe(&data, name, target_ns),
                    "C lz4 (dict 2K)" => {
                        bench_c_lz4_dict(&data, dict_data.as_ref().unwrap(), name, target_ns)
                    }
                    _ => unreachable!(),
                }
            };
            results.push(r);
        }
    }

    save_cache(&results, codecs);
}
