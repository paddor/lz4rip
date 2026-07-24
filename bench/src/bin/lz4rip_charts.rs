#![allow(clippy::too_many_arguments)]

use plotters::coord::Shift;
use plotters::prelude::*;
use plotters::style::text_anchor::{HPos, Pos, VPos};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::path::{Path, PathBuf};

const BG: RGBColor = RGBColor(0x0d, 0x11, 0x17);
const GRID: RGBColor = RGBColor(0x21, 0x26, 0x2d);
const AXIS: RGBColor = RGBColor(0x30, 0x36, 0x3d);
const TEXT: RGBColor = RGBColor(0xe6, 0xed, 0xf3);
const MUTED: RGBColor = RGBColor(0x7d, 0x85, 0x90);
const FAINT: RGBColor = RGBColor(0x48, 0x4f, 0x58);

const FONT_BUMP: u32 = 1;
const HEADER_SUBTITLE_OFFSET: i32 = 18;
const LEGEND_ROW_H: f64 = 20.0;
const TRANSFER_RATE: f64 = 1e9;

const SILESIA: &[&str] = &[
    "dickens", "mozilla", "mr", "nci", "ooffice", "osdb", "reymont", "samba", "sao", "webster",
    "x-ray", "xml",
];
const COMPRESSIBLE: &[&str] = &[
    "dickens", "mozilla", "nci", "ooffice", "osdb", "reymont", "samba", "webster", "xml",
];
const INCOMPRESSIBLE: &[&str] = &["mr", "sao", "x-ray"];

const MAIN_CODEC_ORDER: &[&str] = &[
    "C lz4",
    "lz4rip",
    "lz4rip paranoid",
    "lz4_flex unsafe",
    "lz4_flex",
];
const DICT_CODEC_ORDER: &[&str] = &["C lz4 (dict 2K)", "lz4rip (dict 2K)"];
const SWEEP_CODEC_ORDER: &[&str] = &["C lz4", "C lz4 (dict)", "lz4rip", "lz4rip (dict)"];
const STRUCTURED_CODEC_ORDER: &[&str] = &["C lz4", "lz4rip", "lz4_flex unsafe", "lz4_flex"];
const STRUCTURED_DICT_CODEC_ORDER: &[&str] = &["C lz4 (dict 2K)", "lz4rip (dict 2K)"];

const SWEEP_SIZES: &[usize] = &[
    64, 128, 256, 512, 1024, 2048, 4096, 8192, 16384, 32768, 65536, 131072, 262144, 524288, 1048576,
];
const STRUCTURED_SIZES: &[usize] = &[256, 512, 1024, 2048, 4096, 8192];

#[derive(Clone)]
struct CodecStyle {
    key: &'static str,
    label: &'static str,
    color: RGBColor,
    dim: RGBColor,
}

struct Config {
    target: String,
    hw_label: Option<String>,
    styles: Vec<CodecStyle>,
}

#[derive(Deserialize, Clone)]
struct BenchRow {
    codec: String,
    input: String,
    input_size: usize,
    compressed_size: usize,
    compress_ns: f64,
    decompress_ns: f64,
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse()?;
    let cfg = Config::new();
    let out_dir = args.output_dir.unwrap_or_else(|| {
        let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("doc");
        p.push("charts");
        p.push(&cfg.target);
        p
    });

    match args.chart {
        ChartKind::All => {
            std::fs::create_dir_all(&out_dir)?;
            draw_pipeline(&cfg, &out_dir)?;
            draw_summary(&cfg, &out_dir)?;
            draw_dict2k(&cfg, &out_dir)?;
            draw_sweep(&cfg, &out_dir)?;
            let structured_dir = out_dir.join("structured");
            std::fs::create_dir_all(&structured_dir)?;
            draw_structured(&cfg, &structured_dir, false)?;
            draw_structured(&cfg, &structured_dir, true)?;
        }
        ChartKind::Pipeline => {
            std::fs::create_dir_all(&out_dir)?;
            draw_pipeline(&cfg, &out_dir)?;
        }
        ChartKind::Summary => {
            std::fs::create_dir_all(&out_dir)?;
            draw_summary(&cfg, &out_dir)?;
        }
        ChartKind::Dict2k => {
            std::fs::create_dir_all(&out_dir)?;
            draw_dict2k(&cfg, &out_dir)?;
        }
        ChartKind::Sweep => {
            std::fs::create_dir_all(&out_dir)?;
            draw_sweep(&cfg, &out_dir)?;
        }
        ChartKind::Structured => {
            std::fs::create_dir_all(&out_dir)?;
            draw_structured(&cfg, &out_dir, false)?;
            draw_structured(&cfg, &out_dir, true)?;
        }
        ChartKind::StructuredNoDict => {
            std::fs::create_dir_all(&out_dir)?;
            draw_structured(&cfg, &out_dir, false)?;
        }
        ChartKind::StructuredDict2k => {
            std::fs::create_dir_all(&out_dir)?;
            draw_structured(&cfg, &out_dir, true)?;
        }
    }

    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ChartKind {
    All,
    Pipeline,
    Summary,
    Dict2k,
    Sweep,
    Structured,
    StructuredNoDict,
    StructuredDict2k,
}

struct Args {
    chart: ChartKind,
    output_dir: Option<PathBuf>,
}

impl Args {
    fn parse() -> Result<Self, Box<dyn Error>> {
        let mut chart = ChartKind::All;
        let mut output_dir = None;

        for arg in std::env::args().skip(1) {
            if arg == "-h" || arg == "--help" {
                print_help();
                std::process::exit(0);
            }
            if let Some(kind) = parse_chart_kind(&arg) {
                chart = kind;
            } else {
                output_dir = Some(PathBuf::from(arg));
            }
        }

        Ok(Self { chart, output_dir })
    }
}

fn print_help() {
    println!(
        "Usage: lz4rip_charts [all|summary|pipeline|dict2k|sweep|structured|structured-no-dict|structured-dict2k] [OUT_DIR]"
    );
}

fn parse_chart_kind(s: &str) -> Option<ChartKind> {
    match s {
        "all" => Some(ChartKind::All),
        "summary" => Some(ChartKind::Summary),
        "pipeline" => Some(ChartKind::Pipeline),
        "dict2k" => Some(ChartKind::Dict2k),
        "sweep" => Some(ChartKind::Sweep),
        "structured" => Some(ChartKind::Structured),
        "structured-no-dict" | "structured_no_dict" => Some(ChartKind::StructuredNoDict),
        "structured-dict2k" | "structured_dict2k" => Some(ChartKind::StructuredDict2k),
        _ => None,
    }
}

impl Config {
    fn new() -> Self {
        Self {
            target: std::env::consts::ARCH.into(),
            hw_label: detect_hardware(),
            styles: vec![
                codec("C lz4", "lz4 (C)", 0x60a5fa, 0x4680c4),
                codec("lz4rip", "lz4rip (encapsulated unsafe)", 0xf87171, 0xc45050),
                codec(
                    "lz4rip paranoid",
                    "lz4rip paranoid (safe)",
                    0xf472b6,
                    0xc05a92,
                ),
                codec("lz4_flex unsafe", "lz4_flex (unsafe)", 0xf59e0b, 0xc47d08),
                codec("lz4_flex", "lz4_flex (safe)", 0x4ade80, 0x3aaf60),
                codec("C lz4 (dict 2K)", "lz4 (C, dict)", 0x60a5fa, 0x4680c4),
                codec("lz4rip (dict 2K)", "lz4rip (dict)", 0xf87171, 0xc45050),
                codec("C lz4 (dict)", "lz4 (C) + dict", 0x2dd4bf, 0x1f9b8a),
                codec("lz4rip (dict)", "lz4rip + dict", 0xfb923c, 0xc46f2d),
            ],
        }
    }

    fn style(&self, key: &str) -> Option<&CodecStyle> {
        self.styles.iter().find(|s| s.key == key)
    }
}

fn codec(key: &'static str, label: &'static str, color: u32, dim: u32) -> CodecStyle {
    CodecStyle {
        key,
        label,
        color: hex_color(color),
        dim: hex_color(dim),
    }
}

fn hex_color(v: u32) -> RGBColor {
    RGBColor(
        ((v >> 16) & 0xff) as u8,
        ((v >> 8) & 0xff) as u8,
        (v & 0xff) as u8,
    )
}

fn detect_hardware() -> Option<String> {
    let hw_conf = read_chart_hw();
    let mut cpu = std::env::var("LZ4RIP_CPU").ok();
    if cpu.is_none() && cfg!(target_os = "macos") {
        cpu = std::process::Command::new("sysctl")
            .args(["-n", "machdep.cpu.brand_string"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
    }
    if cpu.is_none() {
        cpu = std::fs::read_to_string("/proc/cpuinfo")
            .ok()
            .and_then(|content| {
                content
                    .lines()
                    .find(|l| l.starts_with("model name"))
                    .and_then(|l| l.split_once(':'))
                    .map(|(_, v)| {
                        v.trim()
                            .replace("(R)", "")
                            .replace("(TM)", "")
                            .replace("CPU ", "")
                    })
            });
    }

    let mut extras = Vec::new();
    if std::fs::read_to_string("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor")
        .is_ok_and(|s| s.trim() == "performance")
    {
        extras.push("performance governor".to_string());
    }
    for (path, off_val) in [
        ("/sys/devices/system/cpu/intel_pstate/no_turbo", "1"),
        ("/sys/devices/system/cpu/cpufreq/boost", "0"),
    ] {
        if let Ok(s) = std::fs::read_to_string(path) {
            if s.trim() == off_val {
                extras.push("turbo off".to_string());
            }
            break;
        }
    }
    if extras.is_empty()
        && let Ok(hw) = std::env::var("LZ4RIP_HW_EXTRAS")
    {
        extras.extend(
            hw.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
        );
    }
    let postfix = std::env::var("LZ4RIP_HW_POSTFIX")
        .ok()
        .or_else(|| hw_conf.get("postfix").cloned());
    if let Some(postfix) = postfix {
        for value in postfix
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
        {
            if !extras.iter().any(|existing| existing == &value) {
                extras.push(value);
            }
        }
    }

    let prefix = std::env::var("LZ4RIP_HW_PREFIX")
        .ok()
        .or_else(|| hw_conf.get("prefix").cloned());
    let cores = std::thread::available_parallelism()
        .ok()
        .map(std::num::NonZero::get);

    if let Some(cpu) = &mut cpu
        && let Some(cores) = cores
    {
        cpu.push_str(&format!(", {cores} cores"));
    }

    let mut parts = Vec::new();
    if let Some(prefix) = prefix.filter(|s| !s.trim().is_empty()) {
        parts.push(prefix);
    }
    match (cpu, extras.is_empty()) {
        (Some(mut cpu), false) => {
            cpu.push_str(", ");
            cpu.push_str(&extras.join(", "));
            parts.push(cpu);
        }
        (Some(cpu), true) => parts.push(cpu),
        (None, false) => parts.push(extras.join(", ")),
        (None, true) => {}
    }
    (!parts.is_empty()).then(|| parts.join(", "))
}

fn read_chart_hw() -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for path in [Path::new(".chart_hw"), Path::new("../.chart_hw")] {
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                map.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
        break;
    }
    map
}

fn cache_dir(cfg: &Config) -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
        .join(".cache")
        .join("lz4rip")
        .join(&cfg.target)
}

fn load_cache_dir(path: &Path) -> Vec<BenchRow> {
    let mut rows = Vec::new();
    let Ok(entries) = std::fs::read_dir(path) else {
        return rows;
    };
    let mut files = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
        .collect::<Vec<_>>();
    files.sort();

    for file in files {
        let Ok(content) = std::fs::read_to_string(&file) else {
            continue;
        };
        for line in content.lines().map(str::trim).filter(|l| !l.is_empty()) {
            if let Ok(row) = serde_json::from_str::<BenchRow>(line) {
                rows.push(row);
            }
        }
    }
    rows
}

fn select_codecs(rows: &[BenchRow], order: &[&'static str]) -> Vec<&'static str> {
    order
        .iter()
        .copied()
        .filter(|codec| rows.iter().any(|r| r.codec == *codec))
        .collect()
}

fn require_silesia_rows(
    rows: &[BenchRow],
    codecs: &[&str],
    chart: &str,
) -> Result<(), Box<dyn Error>> {
    let mut missing = Vec::new();
    for codec in codecs {
        for input in SILESIA {
            if !rows.iter().any(|r| r.codec == *codec && r.input == *input) {
                missing.push(format!("{codec} {input}"));
            }
        }
    }
    if missing.is_empty() {
        return Ok(());
    }
    let shown = missing.into_iter().take(24).collect::<Vec<_>>().join(", ");
    Err(format!(
        "{chart}: missing required 12-file Silesia cache rows ({shown}). Run `cargo run --release --example lz4rip_bench` first."
    )
    .into())
}

fn main_rows(cfg: &Config) -> Vec<BenchRow> {
    let allowed = SILESIA.iter().copied().collect::<BTreeSet<_>>();
    load_cache_dir(&cache_dir(cfg))
        .into_iter()
        .filter(|r| allowed.contains(r.input.as_str()))
        .collect()
}

fn draw_summary(cfg: &Config, out_dir: &Path) -> Result<(), Box<dyn Error>> {
    let rows = main_rows(cfg);
    let codecs = select_codecs(&rows, MAIN_CODEC_ORDER);
    if codecs.is_empty() {
        return Err(
            "summary: no main cache rows. Run `cargo run --release --example lz4rip_bench` first."
                .into(),
        );
    }
    require_silesia_rows(&rows, &codecs, "summary")?;

    let groups = [
        ("Compressible", COMPRESSIBLE),
        ("Incompressible", INCOMPRESSIBLE),
    ];
    let mut group_data = BTreeMap::new();
    for (group, files) in groups {
        for codec in &codecs {
            let subset = rows
                .iter()
                .filter(|r| r.codec == *codec && files.contains(&r.input.as_str()))
                .cloned()
                .collect::<Vec<_>>();
            if let Some(parts) = aggregate_pipeline(&subset) {
                group_data.insert((group.to_string(), (*codec).to_string()), parts);
            }
        }
    }

    let width = 850;
    let height = 480;
    let x_left = 70.0;
    let x_right = 830.0;
    let plot_w = x_right - x_left;
    let p_top = if cfg.hw_label.is_some() { 62.0 } else { 45.0 };
    let p_bot = 318.0;
    let y_max = group_data
        .values()
        .map(|(a, b, c)| a + b + c)
        .fold(0.0, f64::max)
        * 1.15;

    let path = output_path(out_dir, "summary.svg");
    let area = root(&path, width, height)?;
    chart_header(
        &area,
        width,
        "12-file Silesia: LZ4 Pipeline @1 GB/s aggregate (lower is better)",
        cfg.hw_label.as_deref(),
        22,
    )?;
    vtext(
        &area,
        "seconds / GB",
        22,
        px((p_top + p_bot) / 2.0),
        11,
        TEXT,
    )?;
    draw_y_grid(&area, x_left, x_right, p_top, p_bot, y_max, false)?;

    let group_w = plot_w / groups.len() as f64;
    let bar_w = (group_w * 0.7 / codecs.len() as f64).min(50.0);
    let inner_gap = bar_w * 0.15;
    let group_gap = group_w * 0.2;
    for (gi, (group, _)) in groups.iter().enumerate() {
        let group_x = x_left + gi as f64 * group_w + group_gap / 2.0;
        for (ci, codec) in codecs.iter().enumerate() {
            let Some(parts) = group_data.get(&((*group).to_string(), (*codec).to_string())) else {
                continue;
            };
            let Some(style) = cfg.style(codec) else {
                continue;
            };
            draw_stack(
                &area,
                group_x + ci as f64 * (bar_w + inner_gap / codecs.len() as f64),
                bar_w,
                p_top,
                p_bot,
                y_max,
                *parts,
                style,
            )?;
        }
        text(
            &area,
            *group,
            px(group_x + codecs.len() as f64 * bar_w / 2.0),
            px(p_bot + 20.0),
            11,
            TEXT,
            HPos::Center,
            true,
        )?;
    }

    let leg_y = p_bot + 52.0;
    draw_legend(&area, cfg, &codecs, width as f64 / 2.0 - 200.0, leg_y, 2)?;
    let rows = codecs.len().div_ceil(2);
    draw_segment_legend(
        &area,
        width as f64 / 2.0,
        leg_y + rows as f64 * LEGEND_ROW_H + 18.0,
    )?;
    area.present()?;
    drop(area);
    finish_svg(&path, width, height)?;
    println!("wrote {}", path.display());
    Ok(())
}

fn draw_pipeline(cfg: &Config, out_dir: &Path) -> Result<(), Box<dyn Error>> {
    let rows = main_rows(cfg);
    let codecs = select_codecs(&rows, MAIN_CODEC_ORDER);
    if codecs.is_empty() {
        return Err(
            "pipeline: no main cache rows. Run `cargo run --release --example lz4rip_bench` first."
                .into(),
        );
    }
    require_silesia_rows(&rows, &codecs, "pipeline")?;

    let mut stacks = BTreeMap::new();
    let mut input_sizes = BTreeMap::new();
    for row in &rows {
        if !codecs.iter().any(|c| row.codec == *c) {
            continue;
        }
        stacks.insert(
            (row.input.clone(), row.codec.clone()),
            compute_pipeline(row),
        );
        input_sizes.insert(row.input.clone(), row.input_size);
    }

    let width = 850;
    let height = 760;
    let x_left = 55.0;
    let x_right = 830.0;
    let plot_w = x_right - x_left;
    let panel_h = 240.0;
    let panel_gap = 70.0;
    let top = if cfg.hw_label.is_some() { 62.0 } else { 43.0 };
    let panel_tops = [top, top + panel_h + panel_gap];
    let y_max = stacks
        .values()
        .map(|(a, b, c)| a + b + c)
        .fold(0.0, f64::max)
        * 1.1;

    let path = output_path(out_dir, "pipeline.svg");
    let area = root(&path, width, height)?;
    chart_header(
        &area,
        width,
        "12-file Silesia: Per-file LZ4 pipeline @1 GB/s (lower is better)",
        cfg.hw_label.as_deref(),
        22,
    )?;
    vtext(
        &area,
        "seconds / GB",
        22,
        px((panel_tops[0] + panel_tops[1] + panel_h) / 2.0),
        11,
        TEXT,
    )?;

    let mid = SILESIA.len().div_ceil(2);
    let panels = [&SILESIA[..mid], &SILESIA[mid..]];
    for (pi, panel_inputs) in panels.iter().enumerate() {
        let p_top = panel_tops[pi];
        let p_bot = p_top + panel_h;
        draw_y_grid(&area, x_left, x_right, p_top, p_bot, y_max, false)?;

        let group_w = plot_w / panel_inputs.len() as f64;
        let bar_w = group_w * 0.75 / codecs.len() as f64;
        let gap = group_w * 0.25;
        for (gi, input) in panel_inputs.iter().enumerate() {
            let group_x = x_left + gi as f64 * group_w + gap / 2.0;
            for (ci, codec) in codecs.iter().enumerate() {
                let Some(parts) = stacks.get(&((*input).to_string(), (*codec).to_string())) else {
                    continue;
                };
                let Some(style) = cfg.style(codec) else {
                    continue;
                };
                draw_stack(
                    &area,
                    group_x + ci as f64 * bar_w,
                    bar_w,
                    p_top,
                    p_bot,
                    y_max,
                    *parts,
                    style,
                )?;
            }
            let cx = group_x + codecs.len() as f64 * bar_w / 2.0;
            text(
                &area,
                *input,
                px(cx),
                px(p_bot + 17.0),
                10,
                TEXT,
                HPos::Center,
                true,
            )?;
            text(
                &area,
                human_size(*input_sizes.get(*input).unwrap_or(&0)),
                px(cx),
                px(p_bot + 32.0),
                9,
                MUTED,
                HPos::Center,
                false,
            )?;
        }
    }

    let leg_y = panel_tops[1] + panel_h + 50.0;
    draw_legend(&area, cfg, &codecs, width as f64 / 2.0 - 200.0, leg_y, 2)?;
    let rows = codecs.len().div_ceil(2);
    draw_segment_legend(
        &area,
        width as f64 / 2.0,
        leg_y + rows as f64 * LEGEND_ROW_H + 18.0,
    )?;
    area.present()?;
    drop(area);
    finish_svg(&path, width, height)?;
    println!("wrote {}", path.display());
    Ok(())
}

fn draw_dict2k(cfg: &Config, out_dir: &Path) -> Result<(), Box<dyn Error>> {
    let rows = main_rows(cfg);
    let codecs = select_codecs(&rows, DICT_CODEC_ORDER);
    if codecs.is_empty() {
        return Err("dict2k: no dict cache rows. Run `cargo run --release --example lz4rip_bench -- --dict-silesia` first.".into());
    }
    require_silesia_rows(&rows, &codecs, "dict2k")?;

    let mut data = BTreeMap::new();
    for codec in &codecs {
        let subset = rows
            .iter()
            .filter(|r| r.codec == *codec)
            .cloned()
            .collect::<Vec<_>>();
        if let Some(parts) = aggregate_pipeline(&subset) {
            data.insert((*codec).to_string(), parts);
        }
    }

    let width = 520;
    let height = 360;
    let x_left = 70.0;
    let x_right = 490.0;
    let p_top = if cfg.hw_label.is_some() { 62.0 } else { 45.0 };
    let p_bot = 240.0;
    let plot_w = x_right - x_left;
    let y_max = data.values().map(|(a, b, c)| a + b + c).fold(0.0, f64::max) * 1.15;

    let path = output_path(out_dir, "dict2k.svg");
    let area = root(&path, width, height)?;
    chart_header(
        &area,
        width,
        "12-file Silesia + Dict 2K: Pipeline @1 GB/s (lower is better)",
        cfg.hw_label.as_deref(),
        22,
    )?;
    vtext(
        &area,
        "seconds / GB",
        22,
        px((p_top + p_bot) / 2.0),
        11,
        TEXT,
    )?;
    draw_y_grid(&area, x_left, x_right, p_top, p_bot, y_max, true)?;

    let bar_w = plot_w * 0.24;
    let gap = plot_w * 0.1;
    let total = codecs.len() as f64 * bar_w + (codecs.len() - 1) as f64 * gap;
    let start_x = x_left + (plot_w - total) / 2.0;
    for (ci, codec) in codecs.iter().enumerate() {
        let Some(parts) = data.get(*codec) else {
            continue;
        };
        let Some(style) = cfg.style(codec) else {
            continue;
        };
        let x = start_x + ci as f64 * (bar_w + gap);
        draw_stack(&area, x, bar_w, p_top, p_bot, y_max, *parts, style)?;
        text(
            &area,
            style.label,
            px(x + bar_w / 2.0),
            px(p_bot + 18.0),
            10,
            TEXT,
            HPos::Center,
            true,
        )?;
    }

    draw_segment_legend(&area, width as f64 / 2.0, p_bot + 58.0)?;
    text(
        &area,
        "2 KB dictionary trained from 1 KB slices of each Silesia file",
        px(width as f64 / 2.0),
        px(height as f64 - 18.0),
        9,
        FAINT,
        HPos::Center,
        false,
    )?;
    area.present()?;
    drop(area);
    finish_svg(&path, width, height)?;
    println!("wrote {}", path.display());
    Ok(())
}

fn draw_sweep(cfg: &Config, out_dir: &Path) -> Result<(), Box<dyn Error>> {
    let rows = load_cache_dir(&cache_dir(cfg).join("sweep"));
    let codecs = select_codecs(&rows, SWEEP_CODEC_ORDER);
    if codecs.is_empty() {
        return Err("sweep: no cache rows. Run `cargo run --release --example lz4rip_bench -- --sweep` first.".into());
    }

    let mut grouped: BTreeMap<(String, usize), Vec<BenchRow>> = BTreeMap::new();
    for row in rows {
        if codecs.iter().any(|c| row.codec == *c) {
            grouped
                .entry((row.codec.clone(), row.input_size))
                .or_default()
                .push(row);
        }
    }
    let sizes = SWEEP_SIZES
        .iter()
        .copied()
        .filter(|size| {
            codecs
                .iter()
                .any(|codec| grouped.contains_key(&((*codec).to_string(), *size)))
        })
        .collect::<Vec<_>>();
    if sizes.len() < 2 {
        return Err("sweep: not enough size points in cache".into());
    }

    let width = 850;
    let height = 780;
    let margin_l = 80.0;
    let margin_r = 70.0;
    let margin_top = if cfg.hw_label.is_some() { 62.0 } else { 43.0 };
    let panel_gap = 80.0;
    let panel_h = 255.0;
    let plot_w = width as f64 - margin_l - margin_r;
    let log_min = (sizes[0] as f64).log10();
    let log_max = (*sizes.last().unwrap() as f64).log10();
    let x_pos =
        |size: usize| margin_l + ((size as f64).log10() - log_min) / (log_max - log_min) * plot_w;

    let path = output_path(out_dir, "sweep.svg");
    let area = root(&path, width, height)?;
    chart_header(
        &area,
        width,
        "12-file Silesia Prefix Sweep: LZ4 throughput (log-log)",
        cfg.hw_label.as_deref(),
        22,
    )?;
    let panel1_top = margin_top + 18.0;
    let panel2_top = panel1_top + panel_h + panel_gap;
    draw_sweep_panel(
        cfg,
        &area,
        &grouped,
        &codecs,
        &sizes,
        panel1_top,
        panel_h,
        margin_l,
        plot_w,
        "Compress",
        |r| r.compress_ns,
        &x_pos,
    )?;
    draw_sweep_panel(
        cfg,
        &area,
        &grouped,
        &codecs,
        &sizes,
        panel2_top,
        panel_h,
        margin_l,
        plot_w,
        "Roundtrip (compress + decompress)",
        |r| r.compress_ns + r.decompress_ns,
        &x_pos,
    )?;

    text(
        &area,
        "input prefix size",
        px(width as f64 / 2.0),
        px(panel2_top + panel_h + 32.0),
        11,
        TEXT,
        HPos::Center,
        true,
    )?;
    draw_sweep_legend(&area, cfg, &codecs, margin_l, panel2_top + panel_h + 56.0)?;
    area.present()?;
    drop(area);
    finish_svg(&path, width, height)?;
    println!("wrote {}", path.display());
    Ok(())
}

fn draw_structured(cfg: &Config, out_dir: &Path, dict: bool) -> Result<(), Box<dyn Error>> {
    let rows = load_cache_dir(&cache_dir(cfg).join("structured"));
    let order = if dict {
        STRUCTURED_DICT_CODEC_ORDER
    } else {
        STRUCTURED_CODEC_ORDER
    };
    let codecs = select_codecs(&rows, order);
    if codecs.is_empty() {
        let cmd = if dict {
            "--structured-dict"
        } else {
            "--structured"
        };
        return Err(format!(
            "structured: no cache rows. Run `cargo run --release --example lz4rip_bench -- {cmd}` first."
        )
        .into());
    }

    let mut grouped: BTreeMap<(usize, String), Vec<BenchRow>> = BTreeMap::new();
    for row in rows {
        if codecs.iter().any(|c| row.codec == *c) && STRUCTURED_SIZES.contains(&row.input_size) {
            grouped
                .entry((row.input_size, row.codec.clone()))
                .or_default()
                .push(row);
        }
    }
    let sizes = STRUCTURED_SIZES
        .iter()
        .copied()
        .filter(|size| {
            codecs
                .iter()
                .any(|codec| grouped.contains_key(&(*size, (*codec).to_string())))
        })
        .collect::<Vec<_>>();
    if sizes.is_empty() {
        return Err("structured: no Silesia prefix rows in cache".into());
    }

    let mut stacks = BTreeMap::new();
    let mut y_max: f64 = 0.0;
    for size in &sizes {
        for codec in &codecs {
            if let Some(rows) = grouped.get(&(*size, (*codec).to_string()))
                && let Some(parts) = aggregate_pipeline(rows)
            {
                y_max = y_max.max(parts.0 + parts.1 + parts.2);
                stacks.insert((*size, (*codec).to_string()), parts);
            }
        }
    }
    y_max *= 1.1;

    let width = 850;
    let height = 510;
    let x_left = 55.0;
    let x_right = 830.0;
    let p_top = if cfg.hw_label.is_some() { 62.0 } else { 45.0 };
    let p_bot = p_top + 340.0;
    let plot_w = x_right - x_left;
    let file_name = if dict { "dict2k.svg" } else { "no_dict.svg" };
    let path = output_path(out_dir, file_name);
    let area = root(&path, width, height)?;
    let title = if dict {
        "12-file Silesia Prefixes + Dict 2K"
    } else {
        "12-file Silesia Prefixes: Compressor Reuse"
    };
    chart_header(
        &area,
        width,
        &format!("{title}: Pipeline @1 GB/s (lower is better)"),
        cfg.hw_label.as_deref(),
        22,
    )?;
    vtext(
        &area,
        "seconds / GB",
        22,
        px((p_top + p_bot) / 2.0),
        11,
        TEXT,
    )?;
    draw_y_grid(&area, x_left, x_right, p_top, p_bot, y_max, false)?;

    let group_w = plot_w / sizes.len() as f64;
    let n_slots = 4.0;
    let bar_w = group_w * 0.75 / n_slots;
    let gap = group_w * 0.25;
    for (gi, size) in sizes.iter().enumerate() {
        let group_x = x_left + gi as f64 * group_w + gap / 2.0;
        for codec in &codecs {
            let slot = slot_for(codec);
            let Some(parts) = stacks.get(&(*size, (*codec).to_string())) else {
                continue;
            };
            let Some(style) = cfg.style(codec) else {
                continue;
            };
            draw_stack(
                &area,
                group_x + slot as f64 * bar_w,
                bar_w,
                p_top,
                p_bot,
                y_max,
                *parts,
                style,
            )?;
        }
        text(
            &area,
            fmt_size(*size),
            px(group_x + n_slots * bar_w / 2.0),
            px(p_bot + 18.0),
            10,
            TEXT,
            HPos::Center,
            true,
        )?;
    }

    let leg_y = p_bot + 42.0;
    draw_legend(&area, cfg, &codecs, width as f64 / 2.0 - 200.0, leg_y, 2)?;
    let rows = codecs.len().div_ceil(2);
    draw_segment_legend(
        &area,
        width as f64 / 2.0,
        leg_y + rows as f64 * LEGEND_ROW_H + 18.0,
    )?;
    area.present()?;
    drop(area);
    finish_svg(&path, width, height)?;
    println!("wrote {}", path.display());
    Ok(())
}

fn draw_sweep_panel<F>(
    cfg: &Config,
    area: &Area<'_>,
    grouped: &BTreeMap<(String, usize), Vec<BenchRow>>,
    codecs: &[&str],
    sizes: &[usize],
    panel_top: f64,
    panel_h: f64,
    margin_l: f64,
    plot_w: f64,
    title: &str,
    get_ns: F,
    x_pos: &dyn Fn(usize) -> f64,
) -> Result<(), Box<dyn Error>>
where
    F: Fn(&BenchRow) -> f64,
{
    let y_bot = panel_top + panel_h;
    let x_right = margin_l + plot_w;
    let mut metrics: BTreeMap<(String, usize), (f64, f64)> = BTreeMap::new();
    let mut all_ops = Vec::new();
    let mut all_mbs = Vec::new();

    for codec in codecs {
        for size in sizes {
            let Some(rows) = grouped.get(&((*codec).to_string(), *size)) else {
                continue;
            };
            let ops = geomean(
                rows.iter()
                    .filter_map(|r| (get_ns(r) > 0.0).then(|| 1e9 / get_ns(r))),
            );
            let mbs = geomean(rows.iter().filter_map(|r| {
                (get_ns(r) > 0.0).then(|| r.input_size as f64 / get_ns(r) * 1000.0)
            }));
            if let (Some(ops), Some(mbs)) = (ops, mbs) {
                all_ops.push(ops);
                all_mbs.push(mbs);
                metrics.insert(((*codec).to_string(), *size), (ops, mbs));
            }
        }
    }
    if metrics.is_empty() {
        return Ok(());
    }

    let ops_min = all_ops.iter().copied().fold(f64::INFINITY, f64::min) * 0.8;
    let ops_max = all_ops.iter().copied().fold(0.0, f64::max) * 1.15;
    let mbs_min = all_mbs.iter().copied().fold(f64::INFINITY, f64::min) * 0.8;
    let mbs_max = all_mbs.iter().copied().fold(0.0, f64::max) * 1.15;
    let ops_log_min = ops_min.log10();
    let ops_log_range = ops_max.log10() - ops_log_min;
    let mbs_log_min = mbs_min.log10();
    let mbs_log_range = mbs_max.log10() - mbs_log_min;
    let y_ops = |v: f64| y_bot - (v.log10() - ops_log_min) / ops_log_range * panel_h;
    let y_mbs = |v: f64| y_bot - (v.log10() - mbs_log_min) / mbs_log_range * panel_h;

    text(
        area,
        title,
        px(425.0),
        px(panel_top - 13.0),
        12,
        TEXT,
        HPos::Center,
        true,
    )?;
    vtext(
        area,
        "ops/sec",
        px(margin_l - 55.0),
        px(panel_top + panel_h / 2.0),
        10,
        TEXT,
    )?;
    let font = ("sans-serif", 10 + FONT_BUMP)
        .into_font()
        .style(FontStyle::Bold)
        .transform(FontTransform::Rotate90);
    let style = TextStyle::from(font)
        .color(&TEXT)
        .pos(Pos::new(HPos::Center, VPos::Center));
    area.draw(&Text::new(
        "throughput".to_string(),
        (px(x_right + 48.0), px(panel_top + panel_h / 2.0)),
        style,
    ))?;
    line(area, margin_l, y_bot, x_right, y_bot, AXIS, 2)?;
    line(area, margin_l, panel_top, margin_l, y_bot, AXIS, 1)?;
    line(area, x_right, panel_top, x_right, y_bot, AXIS, 1)?;

    for size in sizes {
        let xx = x_pos(*size);
        line(area, xx, y_bot, xx, y_bot + 4.0, FAINT, 1)?;
        text(
            area,
            fmt_size(*size),
            px(xx),
            px(y_bot + 18.0),
            9,
            MUTED,
            HPos::Center,
            false,
        )?;
    }

    for tick in log_ticks(ops_min, ops_max) {
        let yy = y_ops(tick);
        if yy < panel_top || yy > y_bot {
            continue;
        }
        line(area, margin_l, yy, x_right, yy, GRID, 1)?;
        text(
            area,
            fmt_ops(tick),
            px(margin_l - 7.0),
            px(yy),
            9,
            MUTED,
            HPos::Right,
            false,
        )?;
    }
    for tick in log_ticks(mbs_min, mbs_max) {
        let yy = y_mbs(tick);
        if yy < panel_top || yy > y_bot {
            continue;
        }
        text(
            area,
            fmt_throughput(tick),
            px(x_right + 7.0),
            px(yy),
            9,
            MUTED,
            HPos::Left,
            false,
        )?;
    }

    for codec in codecs {
        let Some(style) = cfg.style(codec) else {
            continue;
        };
        let mut pts_ops = Vec::new();
        let mut pts_mbs = Vec::new();
        for size in sizes {
            if let Some((ops, mbs)) = metrics.get(&((*codec).to_string(), *size)) {
                pts_ops.push((x_pos(*size), y_ops(*ops)));
                pts_mbs.push((x_pos(*size), y_mbs(*mbs)));
            }
        }
        polyline(area, &pts_ops, style.color, 1, 0.55, true)?;
        polyline(area, &pts_mbs, style.color, 2, 1.0, false)?;
        for (x, y) in pts_mbs {
            dot(area, x, y, 2, style.color)?;
        }
    }
    Ok(())
}

fn aggregate_pipeline(rows: &[BenchRow]) -> Option<(f64, f64, f64)> {
    let total_input: usize = rows.iter().map(|r| r.input_size).sum();
    if total_input == 0 {
        return None;
    }
    let total_compressed: usize = rows.iter().map(|r| r.compressed_size).sum();
    let total_compress_ns: f64 = rows.iter().map(|r| r.compress_ns).sum();
    let total_decompress_ns: f64 = rows.iter().map(|r| r.decompress_ns).sum();
    let per_gb = 1e9 / total_input as f64;
    Some((
        total_compress_ns / 1e9 * per_gb,
        (total_compressed as f64 / total_input as f64) * (1e9 / TRANSFER_RATE),
        total_decompress_ns / 1e9 * per_gb,
    ))
}

fn compute_pipeline(row: &BenchRow) -> (f64, f64, f64) {
    let per_gb = 1e9 / row.input_size as f64;
    (
        row.compress_ns / 1e9 * per_gb,
        (row.compressed_size as f64 / row.input_size as f64) * (1e9 / TRANSFER_RATE),
        row.decompress_ns / 1e9 * per_gb,
    )
}

fn slot_for(codec: &str) -> usize {
    match codec {
        "C lz4" | "C lz4 (dict 2K)" => 0,
        "lz4rip" | "lz4rip (dict 2K)" => 1,
        "lz4_flex unsafe" => 2,
        "lz4_flex" => 3,
        _ => 0,
    }
}

fn geomean(values: impl Iterator<Item = f64>) -> Option<f64> {
    let mut n = 0usize;
    let mut sum = 0.0;
    for v in values {
        if v <= 0.0 {
            continue;
        }
        n += 1;
        sum += v.ln();
    }
    (n > 0).then(|| (sum / n as f64).exp())
}

fn output_path(out_dir: &Path, name: &str) -> PathBuf {
    out_dir.join(name)
}

type Area<'a> = DrawingArea<SVGBackend<'a>, Shift>;

fn root(path: &Path, width: u32, height: u32) -> Result<Area<'_>, Box<dyn Error>> {
    let area = SVGBackend::new(path, (width, height)).into_drawing_area();
    area.fill(&BG)?;
    Ok(area)
}

fn finish_svg(path: &Path, width: u32, height: u32) -> Result<(), Box<dyn Error>> {
    let mut svg = std::fs::read_to_string(path)?;
    svg = svg.replacen(
        &format!("<svg width=\"{width}\" height=\"{height}\" viewBox=\"0 0 {width} {height}\""),
        &format!("<svg viewBox=\"0 0 {width} {height}\""),
        1,
    );
    svg = svg.replacen(
        "xmlns=\"http://www.w3.org/2000/svg\"",
        "xmlns=\"http://www.w3.org/2000/svg\" font-family=\"system-ui, -apple-system, sans-serif\"",
        1,
    );
    std::fs::write(path, svg)?;
    Ok(())
}

fn text(
    area: &Area<'_>,
    s: impl Into<String>,
    x: i32,
    y: i32,
    size: u32,
    color: RGBColor,
    hpos: HPos,
    bold: bool,
) -> Result<(), Box<dyn Error>> {
    let mut font = ("sans-serif", size + FONT_BUMP).into_font();
    if bold {
        font = font.style(FontStyle::Bold);
    }
    let style = TextStyle::from(font)
        .color(&color)
        .pos(Pos::new(hpos, VPos::Center));
    area.draw(&Text::new(s.into(), (x, y), style))?;
    Ok(())
}

fn vtext(
    area: &Area<'_>,
    s: &str,
    x: i32,
    y: i32,
    size: u32,
    color: RGBColor,
) -> Result<(), Box<dyn Error>> {
    let font = ("sans-serif", size + FONT_BUMP)
        .into_font()
        .style(FontStyle::Bold)
        .transform(FontTransform::Rotate270);
    let style = TextStyle::from(font)
        .color(&color)
        .pos(Pos::new(HPos::Center, VPos::Center));
    area.draw(&Text::new(s.to_string(), (x, y), style))?;
    Ok(())
}

fn rect(
    area: &Area<'_>,
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    color: RGBColor,
) -> Result<(), Box<dyn Error>> {
    area.draw(&Rectangle::new(
        [(px(x1), px(y1)), (px(x2), px(y2))],
        ShapeStyle::from(&color).filled(),
    ))?;
    Ok(())
}

fn line(
    area: &Area<'_>,
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    color: RGBColor,
    width: u32,
) -> Result<(), Box<dyn Error>> {
    area.draw(&PathElement::new(
        vec![(px(x1), px(y1)), (px(x2), px(y2))],
        color.stroke_width(width),
    ))?;
    Ok(())
}

fn polyline(
    area: &Area<'_>,
    points: &[(f64, f64)],
    color: RGBColor,
    width: u32,
    alpha: f64,
    dashed: bool,
) -> Result<(), Box<dyn Error>> {
    if points.len() < 2 {
        return Ok(());
    }
    if dashed {
        for pair in points.windows(2) {
            dashed_line(area, pair[0], pair[1], color, width)?;
        }
    } else {
        area.draw(&PathElement::new(
            points
                .iter()
                .map(|&(x, y)| (px(x), px(y)))
                .collect::<Vec<_>>(),
            color.mix(alpha).stroke_width(width),
        ))?;
    }
    Ok(())
}

fn dashed_line(
    area: &Area<'_>,
    a: (f64, f64),
    b: (f64, f64),
    color: RGBColor,
    width: u32,
) -> Result<(), Box<dyn Error>> {
    let dx = b.0 - a.0;
    let dy = b.1 - a.1;
    let len = (dx * dx + dy * dy).sqrt();
    if len == 0.0 {
        return Ok(());
    }
    let dash = 5.0;
    let gap = 4.0;
    let mut pos = 0.0;
    while pos < len {
        let end = (pos + dash).min(len);
        let t1 = pos / len;
        let t2 = end / len;
        line(
            area,
            a.0 + dx * t1,
            a.1 + dy * t1,
            a.0 + dx * t2,
            a.1 + dy * t2,
            color,
            width,
        )?;
        pos += dash + gap;
    }
    Ok(())
}

fn dot(area: &Area<'_>, x: f64, y: f64, r: i32, color: RGBColor) -> Result<(), Box<dyn Error>> {
    area.draw(&Circle::new(
        (px(x), px(y)),
        r,
        ShapeStyle::from(&color).filled(),
    ))?;
    Ok(())
}

fn draw_stack(
    area: &Area<'_>,
    x: f64,
    width: f64,
    p_top: f64,
    p_bot: f64,
    y_max: f64,
    parts: (f64, f64, f64),
    style: &CodecStyle,
) -> Result<(), Box<dyn Error>> {
    let (comp, transfer, decomp) = parts;
    let map_y = |v: f64| p_bot - (v / y_max) * (p_bot - p_top);
    rect(area, x, map_y(comp), x + width, p_bot, style.color)?;
    rect(
        area,
        x,
        map_y(comp + transfer),
        x + width,
        map_y(comp),
        style.dim,
    )?;
    rect(
        area,
        x,
        map_y(comp + transfer + decomp),
        x + width,
        map_y(comp + transfer),
        style.color,
    )?;
    Ok(())
}

fn draw_y_grid(
    area: &Area<'_>,
    x_left: f64,
    x_right: f64,
    p_top: f64,
    p_bot: f64,
    y_max: f64,
    one_decimal: bool,
) -> Result<(), Box<dyn Error>> {
    let map_y = |v: f64| p_bot - (v / y_max) * (p_bot - p_top);
    let step = nice_step(y_max, 5);
    let mut v = step;
    while v <= y_max {
        let yy = map_y(v);
        line(area, x_left, yy, x_right, yy, GRID, 1)?;
        let label = if one_decimal {
            format!("{v:.1}")
        } else {
            format!("{v:.0}")
        };
        text(
            area,
            label,
            px(x_left - 8.0),
            px(yy),
            10,
            MUTED,
            HPos::Right,
            false,
        )?;
        v += step;
    }
    line(area, x_left, p_bot, x_right, p_bot, AXIS, 2)?;
    Ok(())
}

fn chart_header(
    area: &Area<'_>,
    width: u32,
    title: &str,
    hw: Option<&str>,
    y: i32,
) -> Result<(), Box<dyn Error>> {
    let mid = (width / 2) as i32;
    text(area, title, mid, y, 14, TEXT, HPos::Center, true)?;
    if let Some(hw) = hw {
        text(
            area,
            hw,
            mid,
            y + HEADER_SUBTITLE_OFFSET,
            10,
            MUTED,
            HPos::Center,
            false,
        )?;
    }
    Ok(())
}

fn draw_legend(
    area: &Area<'_>,
    cfg: &Config,
    items: &[&str],
    x: f64,
    y: f64,
    columns: usize,
) -> Result<(), Box<dyn Error>> {
    let rows = items.len().div_ceil(columns);
    for (i, key) in items.iter().enumerate() {
        let col = i / rows;
        let row = i % rows;
        let Some(style) = cfg.style(key) else {
            continue;
        };
        let lx = x + col as f64 * 230.0;
        let ly = y + row as f64 * LEGEND_ROW_H;
        rect(area, lx, ly - 6.0, lx + 12.0, ly + 6.0, style.color)?;
        text(
            area,
            style.label,
            px(lx + 18.0),
            px(ly),
            10,
            TEXT,
            HPos::Left,
            false,
        )?;
    }
    Ok(())
}

fn draw_segment_legend(area: &Area<'_>, mid_x: f64, y: f64) -> Result<(), Box<dyn Error>> {
    text(
        area,
        "bright = compress + decompress",
        px(mid_x - 210.0),
        px(y),
        9,
        TEXT,
        HPos::Left,
        false,
    )?;
    text(
        area,
        "dim = transfer @1 GB/s",
        px(mid_x + 30.0),
        px(y),
        9,
        MUTED,
        HPos::Left,
        false,
    )?;
    Ok(())
}

fn draw_sweep_legend(
    area: &Area<'_>,
    cfg: &Config,
    codecs: &[&str],
    x: f64,
    y: f64,
) -> Result<(), Box<dyn Error>> {
    let col_w = 165.0;
    for (i, codec) in codecs.iter().enumerate() {
        let Some(style) = cfg.style(codec) else {
            continue;
        };
        let lx = x + i as f64 * col_w;
        line(area, lx, y, lx + 20.0, y, style.color, 2)?;
        text(
            area,
            sweep_label(codec),
            px(lx + 26.0),
            px(y),
            10,
            TEXT,
            HPos::Left,
            false,
        )?;
    }
    let style_y = y + 24.0;
    line(area, x, style_y, x + 20.0, style_y, MUTED, 2)?;
    text(
        area,
        "solid = throughput (right axis)",
        px(x + 26.0),
        px(style_y),
        9,
        MUTED,
        HPos::Left,
        false,
    )?;
    dashed_line(area, (x + 250.0, style_y), (x + 270.0, style_y), MUTED, 1)?;
    text(
        area,
        "thin = ops/sec (left axis)",
        px(x + 276.0),
        px(style_y),
        9,
        MUTED,
        HPos::Left,
        false,
    )?;
    Ok(())
}

fn px(v: f64) -> i32 {
    v.round() as i32
}

fn nice_step(max_val: f64, target_lines: usize) -> f64 {
    if max_val <= 0.0 {
        return 1.0;
    }
    let raw = max_val / target_lines as f64;
    let mag = 10.0_f64.powf(raw.max(1e-9).log10().floor());
    for s in [1.0, 2.0, 5.0, 10.0] {
        let step = s * mag;
        if max_val / step <= target_lines as f64 + 1.0 {
            return step;
        }
    }
    mag * 10.0
}

fn log_ticks(min_val: f64, max_val: f64) -> Vec<f64> {
    let lo = min_val.log10().floor() as i32;
    let hi = max_val.log10().ceil() as i32;
    let mut ticks = Vec::new();
    for exp in lo..=hi {
        for mult in [1.0, 2.0, 5.0] {
            let tick = mult * 10.0_f64.powi(exp);
            if min_val <= tick && tick <= max_val {
                ticks.push(tick);
            }
        }
    }
    ticks
}

fn human_size(n: usize) -> String {
    if n >= 1_000_000 {
        format!("{:.1} MB", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.0} KB", n as f64 / 1_000.0)
    } else {
        format!("{n} B")
    }
}

fn fmt_size(n: usize) -> String {
    if n >= 1_048_576 {
        format!("{}M", n / 1_048_576)
    } else if n >= 1024 {
        format!("{}K", n / 1024)
    } else {
        n.to_string()
    }
}

fn fmt_ops(v: f64) -> String {
    if v >= 1e6 {
        format!("{:.0}M", v / 1e6)
    } else if v >= 1e3 {
        format!("{:.0}K", v / 1e3)
    } else {
        format!("{v:.0}")
    }
}

fn fmt_throughput(v: f64) -> String {
    if v >= 1e3 {
        format!("{:.0} GB/s", v / 1e3)
    } else {
        format!("{v:.0} MB/s")
    }
}

fn sweep_label(codec: &str) -> &'static str {
    match codec {
        "C lz4" => "lz4 (C)",
        "C lz4 (dict)" => "lz4 (C) + dict",
        "lz4rip" => "lz4rip",
        "lz4rip (dict)" => "lz4rip + dict",
        _ => "unknown",
    }
}
