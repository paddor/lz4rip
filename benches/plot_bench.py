#!/usr/bin/env python3
"""Generate benchmark charts from cached results.

Results are read from ~/.cache/lz4rip/<arch>/ (written by lz4rip_bench).

Usage:
    python3 benches/plot_bench.py doc/charts/x86_64          # all charts
    python3 benches/plot_bench.py --sweep doc/charts/x86_64  # sweep only
    python3 benches/plot_bench.py --structured doc/charts/x86_64/structured
"""

import json
import os
import random
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


CODEC_ORDER = ["C lz4", "lz4rip", "lz4_flex unsafe", "lz4_flex"]

COLORS = {
    "C lz4":             ("#60a5fa", "#4680c4"),   # blue
    "lz4rip":            ("#f87171", "#c45050"),   # red
    "lz4_flex unsafe":   ("#f59e0b", "#c47d08"),   # amber
    "lz4_flex":          ("#4ade80", "#3aaf60"),   # green
}

LABELS = {
    "C lz4":             "lz4 (C)",
    "lz4rip":            "lz4rip (safe)",
    "lz4_flex unsafe":   "lz4_flex (unsafe)",
    "lz4_flex":          "lz4_flex (safe)",
}

DICT_CODEC_ORDER = ["C lz4 (dict 2K)", "lz4rip (dict 2K)"]

DICT_COLORS = {
    "C lz4 (dict 2K)":   ("#60a5fa", "#4680c4"),   # blue
    "lz4rip (dict 2K)":  ("#f87171", "#c45050"),   # red
}

DICT_LABELS = {
    "C lz4 (dict 2K)":   "lz4 (C)",
    "lz4rip (dict 2K)":  "lz4rip (safe)",
}

MAIN_INPUTS = {
    "dickens", "mozilla", "mr", "nci", "ooffice", "osdb", "reymont",
    "samba", "sao", "webster", "x-ray", "xml", "hdfs.json",
}


def human_size(n):
    if n >= 1_000_000:
        return f"{n / 1_000_000:.1f} MB"
    if n >= 1_000:
        return f"{n / 1_000:.0f} KB"
    return f"{n} B"


def load_results(path):
    text = Path(path).read_text()
    text = text.strip()
    if text.startswith("["):
        return json.loads(text)
    return [json.loads(line) for line in text.splitlines() if line.strip()]


def get_inputs(results):
    seen = set()
    inputs = []
    for r in results:
        if r["input"] not in seen:
            inputs.append(r["input"])
            seen.add(r["input"])
    return inputs


def nice_step(max_val, target_lines):
    raw = max_val / target_lines
    mag = 10 ** int(f"{raw:.0e}".split("e")[1])
    for s in [1, 2, 5, 10]:
        step = s * mag
        if max_val / step <= target_lines + 1:
            return step
    return mag * 10


def detect_hardware():
    try:
        cpu = os.environ.get("LZ4RIP_CPU")
        if not cpu:
            for line in open("/proc/cpuinfo"):
                if line.startswith("model name"):
                    cpu = line.split(":", 1)[1].strip()
                    cpu = cpu.replace("(R)", "").replace("(TM)", "").replace("CPU ", "")
                    break
        if cpu:
            label = cpu
            extras = []
            try:
                gov = open("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor").read().strip()
                if gov == "performance":
                    extras.append("performance governor")
            except OSError:
                pass
            for path, off_val in [
                ("/sys/devices/system/cpu/intel_pstate/no_turbo", "1"),
                ("/sys/devices/system/cpu/cpufreq/boost", "0"),
            ]:
                try:
                    if open(path).read().strip() == off_val:
                        extras.append("turbo off")
                    break
                except OSError:
                    continue
            if not extras:
                hw_extras = os.environ.get("LZ4RIP_HW_EXTRAS")
                if hw_extras:
                    extras.extend(hw_extras.split(","))
            if extras:
                label += ", " + ", ".join(extras)
            return label
    except OSError:
        pass
    return None


def pipeline_chart(results, out_dir):
    codecs = [c for c in CODEC_ORDER if any(r["codec"] == c for r in results)]
    codec_set = set(codecs)
    inputs = [i for i in get_inputs(results)
              if any(r["input"] == i and r["codec"] in codec_set for r in results)]
    n_codecs = len(codecs)

    # split inputs into two panels
    mid = (len(inputs) + 1) // 2
    panels = [inputs[:mid], inputs[mid:]]

    hw_label = detect_hardware()

    svg_w = 850
    x_left, x_right = 55, 830
    plot_w = x_right - x_left
    panel_h = 240
    panel_gap = 70
    top_margin = 50 if hw_label else 40

    panel_tops = [
        top_margin,
        top_margin + panel_h + panel_gap,
    ]
    svg_h = panel_tops[-1] + panel_h + 120

    # compute stacked values: compress + transfer + decompress (seconds per GB)
    # transfer rate: 1000 Mbit/s = 125 MB/s (same assumption as lz4.org)
    transfer_rate = 1e9  # bytes per second (1 GB/s)
    stacks = {}
    y_max = 0
    for inp in inputs:
        for codec in codecs:
            r = next((r for r in results if r["input"] == inp and r["codec"] == codec), None)
            if not r:
                continue
            per_gb = 1e9 / r["input_size"]
            comp = r["compress_ns"] / 1e9 * per_gb
            transfer = (r["compressed_size"] / r["input_size"]) * (1e9 / transfer_rate)
            decomp = r["decompress_ns"] / 1e9 * per_gb
            stacks[(inp, codec)] = (comp, transfer, decomp)
            y_max = max(y_max, comp + transfer + decomp)

    y_max *= 1.1

    mid_x = (x_left + x_right) / 2
    L = []
    L.append(
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {svg_w} {svg_h}"'
        f' font-family="system-ui, -apple-system, sans-serif">'
    )
    L.append(f'  <rect width="{svg_w}" height="{svg_h}" fill="#0d1117"/>')

    # title
    L.append(
        f'  <text x="{mid_x}" y="22" text-anchor="middle" fill="#e6edf3"'
        f' font-size="14" font-weight="700">'
        f'LZ4 Block: Compress + Transfer @1 GB/s + Decompress (lower is better)'
        f'</text>'
    )
    if hw_label:
        L.append(
            f'  <text x="{mid_x}" y="38" text-anchor="middle" fill="#7d8590"'
            f' font-size="10">{hw_label}</text>'
        )

    for pi, panel_inputs in enumerate(panels):
        n_inputs = len(panel_inputs)
        p_top = panel_tops[pi]
        p_bot = p_top + panel_h

        group_w = plot_w / n_inputs
        bar_w = group_w * 0.75 / n_codecs
        gap = group_w * 0.25

        def y(v, _bot=p_bot, _top=p_top):
            return _bot - (v / y_max) * (_bot - _top)

        # y gridlines + labels
        step = nice_step(y_max, 5)
        v = step
        while v <= y_max:
            yy = y(v)
            L.append(
                f'  <line x1="{x_left}" y1="{yy:.1f}" x2="{x_right}" y2="{yy:.1f}"'
                f' stroke="#21262d" stroke-width="1"/>'
            )
            L.append(
                f'  <text x="{x_left - 8}" y="{yy:.1f}" text-anchor="end"'
                f' dominant-baseline="middle" fill="#7d8590" font-size="10">{v:.0f}</text>'
            )
            v += step

        # baseline
        L.append(
            f'  <line x1="{x_left}" y1="{p_bot}" x2="{x_right}" y2="{p_bot}"'
            f' stroke="#30363d" stroke-width="1.5"/>'
        )

        # y-axis label (only on first panel)
        if pi == 0:
            total_mid_y = (panel_tops[0] + panel_tops[1] + panel_h) / 2
            L.append(
                f'  <text x="22" y="{total_mid_y}" text-anchor="middle" fill="#e6edf3"'
                f' font-size="11" font-weight="600"'
                f' transform="rotate(-90,22,{total_mid_y})">seconds / GB</text>'
            )

        # bars
        for gi, inp in enumerate(panel_inputs):
            group_x = x_left + gi * group_w + gap / 2

            for ci, codec in enumerate(codecs):
                if (inp, codec) not in stacks:
                    continue
                comp, transfer, decomp = stacks[(inp, codec)]
                main_c, xfer_c = COLORS[codec]

                bx = group_x + ci * bar_w
                h_comp = (comp / y_max) * (p_bot - p_top)
                L.append(
                    f'  <rect x="{bx:.1f}" y="{y(comp):.1f}"'
                    f' width="{bar_w:.1f}" height="{h_comp:.1f}"'
                    f' fill="{main_c}" rx="1"/>'
                )
                h_transfer = (transfer / y_max) * (p_bot - p_top)
                L.append(
                    f'  <rect x="{bx:.1f}" y="{y(comp + transfer):.1f}"'
                    f' width="{bar_w:.1f}" height="{h_transfer:.1f}"'
                    f' fill="{xfer_c}" rx="1"/>'
                )
                h_decomp = (decomp / y_max) * (p_bot - p_top)
                L.append(
                    f'  <rect x="{bx:.1f}" y="{y(comp + transfer + decomp):.1f}"'
                    f' width="{bar_w:.1f}" height="{h_decomp:.1f}"'
                    f' fill="{main_c}" rx="1"/>'
                )

            # group label
            r0 = next(r for r in results if r["input"] == inp)
            label = inp.replace(".txt", "").replace("compression_", "")
            size_label = human_size(r0["input_size"])
            cx = group_x + (n_codecs * bar_w) / 2
            L.append(
                f'  <text x="{cx:.1f}" y="{p_bot + 16}" text-anchor="middle"'
                f' fill="#e6edf3" font-size="10" font-weight="600">{label}</text>'
            )
            L.append(
                f'  <text x="{cx:.1f}" y="{p_bot + 28}" text-anchor="middle"'
                f' fill="#7d8590" font-size="9">{size_label}</text>'
            )

    # legend: codec colors (2 columns, lz4_flex variants aligned in right column)
    leg_y = panel_tops[-1] + panel_h + 50
    legend_items = [(k, LABELS[k]) for k in codecs if k in COLORS]
    row_h = 18
    # Two columns, column-major: left column takes the extra entry when the
    # count is odd.
    left_count = (len(legend_items) + 1) // 2
    leg_positions = [(0, r) for r in range(left_count)] + [
        (1, r) for r in range(len(legend_items) - left_count)
    ]
    leg_col_x = [mid_x - 200, mid_x + 10]
    for i, (key, label) in enumerate(legend_items):
        if i >= len(leg_positions):
            break
        col, row = leg_positions[i]
        lx = leg_col_x[col]
        ly = leg_y + row * row_h
        main_c, xfer_c = COLORS[key]
        L.append(
            f'  <rect x="{lx:.0f}" y="{ly - 5}" width="12" height="12"'
            f' fill="{main_c}" rx="2"/>'
        )
        L.append(
            f'  <text x="{lx + 18:.0f}" y="{ly + 5}" fill="#e6edf3"'
            f' font-size="10" font-weight="500">{label}</text>'
        )

    n_legend_rows = left_count
    # bar segment legend: bright = compress/decompress, dim = transfer
    seg_y = leg_y + n_legend_rows * row_h + 8
    seg_items = [
        ("bright = compress + decompress", "#e6edf3"),
        ("dim = transfer @1 GB/s", "#7d8590"),
    ]
    seg_total = 420
    seg_start = mid_x - seg_total / 2
    for i, (label, fill) in enumerate(seg_items):
        sx = seg_start + i * 240
        L.append(
            f'  <text x="{sx:.0f}" y="{seg_y + 4}" fill="{fill}"'
            f' font-size="9">{label}</text>'
        )

    L.append("</svg>")
    return "\n".join(L) + "\n"


COMPRESSIBLE = {
    "dickens", "hdfs.json", "mozilla", "nci", "ooffice", "osdb",
    "reymont", "samba", "webster", "xml",
}
INCOMPRESSIBLE = {"mr", "sao", "x-ray"}


def summary_chart(results, out_dir):
    codecs = [c for c in CODEC_ORDER if any(r["codec"] == c for r in results)]
    n_codecs = len(codecs)
    hw_label = detect_hardware()

    transfer_rate = 1e9  # bytes per second (1 GB/s)

    groups = [
        ("Compressible", COMPRESSIBLE),
        ("Incompressible", INCOMPRESSIBLE),
    ]

    # aggregate pipeline time per codec per group (sum bytes, sum ns, derive s/GB)
    group_data = {}
    for group_name, file_set in groups:
        for codec in codecs:
            total_input = 0
            total_compressed = 0
            total_compress_ns = 0
            total_decompress_ns = 0
            for r in results:
                if r["codec"] != codec or r["input"] not in file_set:
                    continue
                total_input += r["input_size"]
                total_compressed += r["compressed_size"]
                total_compress_ns += r["compress_ns"]
                total_decompress_ns += r["decompress_ns"]
            if total_input > 0:
                per_gb = 1e9 / total_input
                comp = total_compress_ns / 1e9 * per_gb
                transfer = (total_compressed / total_input) * (1e9 / transfer_rate)
                decomp = total_decompress_ns / 1e9 * per_gb
                group_data[(group_name, codec)] = (comp, transfer, decomp)

    n_groups = len(groups)
    svg_w = 850
    svg_h = 460
    x_left, x_right = 70, 830
    plot_w = x_right - x_left
    y_top = 55 if hw_label else 45
    y_bot = 310
    plot_h = y_bot - y_top

    y_max = 0
    for v in group_data.values():
        y_max = max(y_max, sum(v))
    y_max *= 1.15

    def y(v):
        return y_bot - (v / y_max) * plot_h

    mid_x = (x_left + x_right) / 2
    L = []
    L.append(
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {svg_w} {svg_h}"'
        f' font-family="system-ui, -apple-system, sans-serif">'
    )
    L.append(f'  <rect width="{svg_w}" height="{svg_h}" fill="#0d1117"/>')

    L.append(
        f'  <text x="{mid_x}" y="22" text-anchor="middle" fill="#e6edf3"'
        f' font-size="14" font-weight="700">'
        f'LZ4 Pipeline @1 GB/s: Aggregate across corpus (lower is better)'
        f'</text>'
    )
    if hw_label:
        L.append(
            f'  <text x="{mid_x}" y="38" text-anchor="middle" fill="#7d8590"'
            f' font-size="10">{hw_label}</text>'
        )

    # y gridlines
    step = nice_step(y_max, 5)
    v = step
    while v <= y_max:
        yy = y(v)
        L.append(
            f'  <line x1="{x_left}" y1="{yy:.1f}" x2="{x_right}" y2="{yy:.1f}"'
            f' stroke="#21262d" stroke-width="1"/>'
        )
        L.append(
            f'  <text x="{x_left - 8}" y="{yy:.1f}" text-anchor="end"'
            f' dominant-baseline="middle" fill="#7d8590" font-size="10">{v:.0f}</text>'
        )
        v += step

    # baseline
    L.append(
        f'  <line x1="{x_left}" y1="{y_bot}" x2="{x_right}" y2="{y_bot}"'
        f' stroke="#30363d" stroke-width="1.5"/>'
    )

    # y-axis label
    mid_y = (y_top + y_bot) / 2
    L.append(
        f'  <text x="22" y="{mid_y}" text-anchor="middle" fill="#e6edf3"'
        f' font-size="11" font-weight="600"'
        f' transform="rotate(-90,22,{mid_y})">seconds / GB</text>'
    )

    # bars: 2 groups, n_codecs bars each, with gap between groups
    group_w = plot_w / n_groups
    bar_w = min(group_w * 0.7 / n_codecs, 50)
    inner_gap = bar_w * 0.15
    group_gap = group_w * 0.2

    for gi, (group_name, _) in enumerate(groups):
        group_x = x_left + gi * group_w + group_gap / 2

        for ci, codec in enumerate(codecs):
            if (group_name, codec) not in group_data:
                continue
            comp, transfer, decomp = group_data[(group_name, codec)]
            main_c, xfer_c = COLORS[codec]

            bx = group_x + ci * (bar_w + inner_gap / n_codecs)
            h_comp = (comp / y_max) * plot_h
            L.append(
                f'  <rect x="{bx:.1f}" y="{y(comp):.1f}"'
                f' width="{bar_w:.1f}" height="{h_comp:.1f}"'
                f' fill="{main_c}" rx="1"/>'
            )
            h_transfer = (transfer / y_max) * plot_h
            L.append(
                f'  <rect x="{bx:.1f}" y="{y(comp + transfer):.1f}"'
                f' width="{bar_w:.1f}" height="{h_transfer:.1f}"'
                f' fill="{xfer_c}" rx="1"/>'
            )
            h_decomp = (decomp / y_max) * plot_h
            L.append(
                f'  <rect x="{bx:.1f}" y="{y(comp + transfer + decomp):.1f}"'
                f' width="{bar_w:.1f}" height="{h_decomp:.1f}"'
                f' fill="{main_c}" rx="1"/>'
            )

        # group label
        cx = group_x + (n_codecs * (bar_w + inner_gap / n_codecs)) / 2
        L.append(
            f'  <text x="{cx:.1f}" y="{y_bot + 18}" text-anchor="middle"'
            f' fill="#e6edf3" font-size="11" font-weight="600">{group_name}</text>'
        )

    # legend: codec colors (2 columns, lz4_flex variants aligned in right column)
    leg_y = y_bot + 40
    legend_items = [(k, LABELS[k]) for k in codecs if k in COLORS]
    row_h = 18
    # Two columns, column-major: left column takes the extra entry when the
    # count is odd.
    left_count = (len(legend_items) + 1) // 2
    leg_positions = [(0, r) for r in range(left_count)] + [
        (1, r) for r in range(len(legend_items) - left_count)
    ]
    leg_col_x = [mid_x - 200, mid_x + 10]
    for i, (key, label) in enumerate(legend_items):
        if i >= len(leg_positions):
            break
        col, row = leg_positions[i]
        lx = leg_col_x[col]
        ly = leg_y + row * row_h
        main_c, xfer_c = COLORS[key]
        L.append(
            f'  <rect x="{lx:.0f}" y="{ly - 5}" width="12" height="12"'
            f' fill="{main_c}" rx="2"/>'
        )
        L.append(
            f'  <text x="{lx + 18:.0f}" y="{ly + 5}" fill="#e6edf3"'
            f' font-size="10" font-weight="500">{label}</text>'
        )

    n_legend_rows = left_count
    # bar segment legend: bright = compress/decompress, dim = transfer
    seg_y = leg_y + n_legend_rows * row_h + 8
    seg_items = [
        ("bright = compress + decompress", "#e6edf3"),
        ("dim = transfer @1 GB/s", "#7d8590"),
    ]
    seg_total = 420
    seg_start = mid_x - seg_total / 2
    for i, (label, fill) in enumerate(seg_items):
        sx = seg_start + i * 240
        L.append(
            f'  <text x="{sx:.0f}" y="{seg_y + 4}" fill="{fill}"'
            f' font-size="9">{label}</text>'
        )

    L.append("</svg>")
    return "\n".join(L) + "\n"


def json_payload(target_bytes, counter_start=0):
    """Generate deterministic NDJSON log data, same pattern as OMQ.rs benchmarks."""
    LEVELS = ["DEBUG", "INFO", "WARN", "ERROR"]
    SERVICES = ["api-gateway", "auth-svc", "order-svc", "payment-svc", "notify-svc"]
    METHODS = ["GET", "POST", "PUT", "DELETE", "PATCH"]
    PATHS = ["/v1/widgets", "/v1/users", "/v1/orders", "/v2/events", "/v1/health"]
    REGIONS = ["us-east-1", "us-west-2", "eu-west-1", "ap-south-1", "eu-central-1"]
    STATUSES = [200, 201, 204, 400, 404, 500, 502, 503]
    MSGS = [
        "request handled successfully",
        "resource created",
        "cache miss, fetched from origin",
        "rate limit approaching threshold",
        "upstream timeout, retrying",
    ]
    out = []
    counter = counter_start
    total = 0
    while total < target_bytes:
        h = (counter * 0x9E3779B1) & 0xFFFFFFFF
        hid = f"{h:08x}"
        level = LEVELS[h % len(LEVELS)]
        service = SERVICES[(h >> 4) % len(SERVICES)]
        method = METHODS[(h >> 8) % len(METHODS)]
        path = PATHS[(h >> 12) % len(PATHS)]
        region = REGIONS[(h >> 16) % len(REGIONS)]
        status = STATUSES[(h >> 20) % len(STATUSES)]
        latency = (h % 500) + 1
        msg = MSGS[(h >> 24) % len(MSGS)]
        line = (
            f'{{"ts":"2026-04-27T12:34:56.{hid}Z","level":"{level}",'
            f'"service":"{service}","trace_id":"{hid}","span_id":"{hid}",'
            f'"user_id":"u-{hid}","method":"{method}",'
            f'"path":"{path}/{hid}","status":{status},'
            f'"latency_ms":{latency},"region":"{region}",'
            f'"host":"{service}-{hid}.svc.cluster.local","msg":"{msg}"}}\n'
        )
        out.append(line)
        total += len(line)
        counter += 1
    result = "".join(out)
    return result[:target_bytes]


def train_dict(sample_dir, dict_path, max_dict_size=2048):
    samples = sorted(str(p) for p in Path(sample_dir).glob("*.json"))
    if not samples:
        print("  no samples for dict training", file=sys.stderr)
        return False
    cmd = ["zstd", "--train", f"--maxdict={max_dict_size}",
           "-o", str(dict_path)] + samples
    result = subprocess.run(cmd, capture_output=True)
    if result.returncode != 0:
        print(f"  zstd --train failed: {result.stderr.decode()}", file=sys.stderr)
        return False
    size = Path(dict_path).stat().st_size
    print(f"  trained dict ({size} bytes) -> {dict_path}", file=sys.stderr)
    return True


def run_dict_bench(dict_path, extra_file):
    cmd = ["cargo", "run", "--release", "--example", "lz4rip_bench", "--",
           "--dict", str(dict_path), "--extra", str(extra_file),
           "--files", Path(extra_file).name]
    has_taskset = shutil.which("taskset") is not None
    if has_taskset:
        cmd = ["taskset", "-c", "0"] + cmd
    print(f"  running dict bench: {Path(extra_file).name}", file=sys.stderr)
    result = subprocess.run(cmd, capture_output=True, text=True)
    if result.returncode != 0:
        print(f"  dict bench failed: {result.stderr}", file=sys.stderr)
        return []
    if result.stderr:
        print(result.stderr, end="", file=sys.stderr)
    import platform
    arch = platform.machine()
    base = cache_base() / arch
    return load_cache_dir(base)


def dict_chart(results, out_dir):
    codecs = [c for c in DICT_CODEC_ORDER if any(r["codec"] == c for r in results)]
    n_codecs = len(codecs)
    hw_label = detect_hardware()
    transfer_rate = 1e9

    r0 = results[0]
    input_label = f"1 KB JSON message, dict trained on {DICT_TRAIN_COUNT} messages"

    bar_data = {}
    for codec in codecs:
        r = next((r for r in results if r["codec"] == codec), None)
        if not r:
            continue
        per_gb = 1e9 / r["input_size"]
        comp = r["compress_ns"] / 1e9 * per_gb
        transfer = (r["compressed_size"] / r["input_size"]) * (1e9 / transfer_rate)
        decomp = r["decompress_ns"] / 1e9 * per_gb
        bar_data[codec] = (comp, transfer, decomp, r)

    svg_w = 400
    svg_h = 340
    x_left, x_right = 70, 370
    plot_w = x_right - x_left
    y_top = 55 if hw_label else 45
    y_bot = 230
    plot_h = y_bot - y_top

    y_max = max(sum(v[:3]) for v in bar_data.values()) * 1.15

    def y(v):
        return y_bot - (v / y_max) * plot_h

    mid_x = (x_left + x_right) / 2
    L = []
    L.append(
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {svg_w} {svg_h}"'
        f' font-family="system-ui, -apple-system, sans-serif">'
    )
    L.append(f'  <rect width="{svg_w}" height="{svg_h}" fill="#0d1117"/>')

    L.append(
        f'  <text x="{mid_x}" y="22" text-anchor="middle" fill="#e6edf3"'
        f' font-size="13" font-weight="700">'
        f'LZ4 + Dict (2 KB): 1 KB JSON (lower is better)'
        f'</text>'
    )
    if hw_label:
        L.append(
            f'  <text x="{mid_x}" y="38" text-anchor="middle" fill="#7d8590"'
            f' font-size="10">{hw_label}</text>'
        )

    step = nice_step(y_max, 5)
    v = step
    while v <= y_max:
        yy = y(v)
        L.append(
            f'  <line x1="{x_left}" y1="{yy:.1f}" x2="{x_right}" y2="{yy:.1f}"'
            f' stroke="#21262d" stroke-width="1"/>'
        )
        L.append(
            f'  <text x="{x_left - 8}" y="{yy:.1f}" text-anchor="end"'
            f' dominant-baseline="middle" fill="#7d8590" font-size="10">{v:.1f}</text>'
        )
        v += step

    L.append(
        f'  <line x1="{x_left}" y1="{y_bot}" x2="{x_right}" y2="{y_bot}"'
        f' stroke="#30363d" stroke-width="1.5"/>'
    )

    mid_y = (y_top + y_bot) / 2
    L.append(
        f'  <text x="22" y="{mid_y}" text-anchor="middle" fill="#e6edf3"'
        f' font-size="11" font-weight="600"'
        f' transform="rotate(-90,22,{mid_y})">seconds / GB</text>'
    )

    bar_w = plot_w * 0.3
    gap = plot_w * 0.1
    total_bars_w = n_codecs * bar_w + (n_codecs - 1) * gap
    start_x = x_left + (plot_w - total_bars_w) / 2

    for ci, codec in enumerate(codecs):
        if codec not in bar_data:
            continue
        comp, transfer, decomp, r = bar_data[codec]
        main_c, xfer_c = DICT_COLORS[codec]

        bx = start_x + ci * (bar_w + gap)

        h_comp = (comp / y_max) * plot_h
        L.append(
            f'  <rect x="{bx:.1f}" y="{y(comp):.1f}"'
            f' width="{bar_w:.1f}" height="{h_comp:.1f}"'
            f' fill="{main_c}" rx="2"/>'
        )
        h_transfer = (transfer / y_max) * plot_h
        L.append(
            f'  <rect x="{bx:.1f}" y="{y(comp + transfer):.1f}"'
            f' width="{bar_w:.1f}" height="{h_transfer:.1f}"'
            f' fill="{xfer_c}" rx="2"/>'
        )
        h_decomp = (decomp / y_max) * plot_h
        L.append(
            f'  <rect x="{bx:.1f}" y="{y(comp + transfer + decomp):.1f}"'
            f' width="{bar_w:.1f}" height="{h_decomp:.1f}"'
            f' fill="{main_c}" rx="2"/>'
        )

        total_ns = r["compress_ns"] + r["decompress_ns"]
        cx = bx + bar_w / 2
        L.append(
            f'  <text x="{cx:.1f}" y="{y(comp + transfer + decomp) - 6:.1f}"'
            f' text-anchor="middle" fill="#e6edf3" font-size="10"'
            f' font-weight="600">{total_ns:.0f} ns</text>'
        )

    # codec labels below bars
    for ci, codec in enumerate(codecs):
        bx = start_x + ci * (bar_w + gap)
        cx = bx + bar_w / 2
        L.append(
            f'  <text x="{cx:.1f}" y="{y_bot + 16}" text-anchor="middle"'
            f' fill="#e6edf3" font-size="10" font-weight="600">'
            f'{DICT_LABELS[codec]}</text>'
        )


    # bar segment legend: bright = compress/decompress, dim = transfer
    # (matches pipeline/summary charts; each codec's main_c is brighter than its xfer_c)
    leg_y = y_bot + 52
    seg_items = [
        ("bright = compress + decompress", "#e6edf3"),
        ("dim = transfer @1 GB/s", "#7d8590"),
    ]
    seg_total = 300
    seg_start = mid_x - seg_total / 2
    for i, (label, fill) in enumerate(seg_items):
        sx = seg_start + i * 170
        L.append(
            f'  <text x="{sx:.0f}" y="{leg_y + 5}" fill="{fill}"'
            f' font-size="9">{label}</text>'
        )

    # subtitle with training info
    L.append(
        f'  <text x="{mid_x}" y="{svg_h - 10}" text-anchor="middle" fill="#484f58"'
        f' font-size="9">{input_label}</text>'
    )

    L.append("</svg>")
    return "\n".join(L) + "\n"


DICT_TRAIN_COUNT = 2000


def generate_dict_charts(out_dir):
    with tempfile.TemporaryDirectory() as tmp:
        # generate training messages (128-2048 bytes each)
        sample_dir = Path(tmp) / "samples"
        sample_dir.mkdir()
        rng = random.Random(42)
        sizes = [rng.randint(128, 2048) for _ in range(DICT_TRAIN_COUNT)]
        for i, size in enumerate(sizes):
            data = json_payload(size, counter_start=i * 100)
            (sample_dir / f"msg_{i:04d}.json").write_text(data)

        # train dict
        dict_path = Path(tmp) / "json.dict"
        if not train_dict(sample_dir, dict_path):
            print("  skipping dict chart: training failed", file=sys.stderr)
            return

        # generate 1 KB bench message
        bench_msg = json_payload(1024, counter_start=999999)
        bench_file = Path(tmp) / "json_1k.json"
        bench_file.write_text(bench_msg)
        print(f"  bench message: {len(bench_msg)} bytes", file=sys.stderr)

        # run benchmark
        results = run_dict_bench(dict_path, bench_file)

    if not results:
        print("  skipping dict chart: no results", file=sys.stderr)
        return

    svg = dict_chart(results, out_dir)
    out_path = out_dir / "dict2k.svg"
    out_path.write_text(svg)
    print(f"  wrote {out_path}")


import math

SWEEP_CODEC_ORDER = ["C lz4", "C lz4 (dict)", "lz4rip", "lz4rip (dict)"]

SWEEP_STYLES = {
    "lz4rip":         {"color": "#f87171"},   # red
    "lz4rip (dict)":  {"color": "#fb923c"},   # orange
    "C lz4":          {"color": "#60a5fa"},   # blue
    "C lz4 (dict)":   {"color": "#2dd4bf"},   # teal
}

SWEEP_LABELS = {
    "lz4rip":         "lz4rip",
    "lz4rip (dict)":  "lz4rip + dict",
    "C lz4":          "lz4 (C)",
    "C lz4 (dict)":   "lz4 (C) + dict",
}


def _fmt_size(n):
    if n >= 1048576:
        return f"{n // 1048576}M"
    if n >= 1024:
        return f"{n // 1024}K"
    return str(n)


def sweep_chart(results):
    hw_label = detect_hardware()
    codecs = [c for c in SWEEP_CODEC_ORDER if any(r["codec"] == c for r in results)]

    # group by codec
    by_codec = {}
    for r in results:
        by_codec.setdefault(r["codec"], []).append(r)
    for v in by_codec.values():
        v.sort(key=lambda r: r["input_size"])

    sizes = sorted(set(r["input_size"] for r in results))
    if len(sizes) < 2:
        return None

    svg_w, svg_h = 850, 760
    margin_l, margin_r = 80, 60
    margin_top = 50 if hw_label else 40
    panel_gap = 80
    panel_h = (svg_h - margin_top - panel_gap - 130) // 2
    plot_w = svg_w - margin_l - margin_r

    log_min = math.log10(sizes[0])
    log_max = math.log10(sizes[-1])

    def x_pos(sz):
        return margin_l + (math.log10(sz) - log_min) / (log_max - log_min) * plot_w

    def make_panel(panel_top, title, get_ns):
        L = []
        y_bot = panel_top + panel_h

        # compute data: ops/sec and MB/s per codec per size
        all_ops = []
        all_mbs = []
        for codec in codecs:
            for r in by_codec.get(codec, []):
                ns = get_ns(r)
                ops = 1e9 / ns
                mbs = r["input_size"] / ns * 1e3
                all_ops.append(ops)
                all_mbs.append(mbs)

        if not all_ops:
            return []

        ops_max = max(all_ops) * 1.15
        mbs_max = max(all_mbs) * 1.15

        # title
        L.append(
            f'  <text x="{svg_w / 2}" y="{panel_top - 12}" text-anchor="middle"'
            f' fill="#e6edf3" font-size="12" font-weight="600">{title}</text>'
        )

        # y-axis left: ops/sec
        L.append(
            f'  <text x="{margin_l - 55}" y="{panel_top + panel_h // 2}"'
            f' text-anchor="middle" fill="#e6edf3" font-size="10" font-weight="600"'
            f' transform="rotate(-90,{margin_l - 55},{panel_top + panel_h // 2})">'
            f'ops/sec</text>'
        )

        # y-axis right: throughput
        L.append(
            f'  <text x="{svg_w - margin_r + 48}" y="{panel_top + panel_h // 2}"'
            f' text-anchor="middle" fill="#e6edf3" font-size="10" font-weight="600"'
            f' transform="rotate(90,{svg_w - margin_r + 48},{panel_top + panel_h // 2})">'
            f'throughput</text>'
        )

        # axes
        L.append(
            f'  <line x1="{margin_l}" y1="{y_bot}" x2="{margin_l + plot_w}" y2="{y_bot}"'
            f' stroke="#30363d" stroke-width="1.5"/>'
        )
        L.append(
            f'  <line x1="{margin_l}" y1="{panel_top}" x2="{margin_l}" y2="{y_bot}"'
            f' stroke="#30363d" stroke-width="1"/>'
        )
        L.append(
            f'  <line x1="{margin_l + plot_w}" y1="{panel_top}" x2="{margin_l + plot_w}" y2="{y_bot}"'
            f' stroke="#30363d" stroke-width="1"/>'
        )

        # x-axis tick labels
        for sz in sizes:
            xx = x_pos(sz)
            L.append(
                f'  <line x1="{xx:.1f}" y1="{y_bot}" x2="{xx:.1f}" y2="{y_bot + 4}"'
                f' stroke="#484f58" stroke-width="1"/>'
            )
            L.append(
                f'  <text x="{xx:.1f}" y="{y_bot + 16}" text-anchor="middle"'
                f' fill="#7d8590" font-size="9">{_fmt_size(sz)}</text>'
            )

        # y gridlines + labels (left: ops/sec, log scale)
        log_ops_min = math.floor(math.log10(min(all_ops)))
        log_ops_max = math.ceil(math.log10(ops_max))
        for exp in range(log_ops_min, log_ops_max + 1):
            val = 10 ** exp
            if val > ops_max:
                break
            yy = y_bot - (math.log10(val) - math.log10(min(all_ops) * 0.8)) / (math.log10(ops_max) - math.log10(min(all_ops) * 0.8)) * panel_h
            if yy < panel_top or yy > y_bot:
                continue
            L.append(
                f'  <line x1="{margin_l}" y1="{yy:.1f}" x2="{margin_l + plot_w}" y2="{yy:.1f}"'
                f' stroke="#21262d" stroke-width="1"/>'
            )
            if val >= 1e6:
                label = f"{val / 1e6:.0f}M"
            elif val >= 1e3:
                label = f"{val / 1e3:.0f}K"
            else:
                label = str(int(val))
            L.append(
                f'  <text x="{margin_l - 6}" y="{yy:.1f}" text-anchor="end"'
                f' dominant-baseline="middle" fill="#7d8590" font-size="9">{label}</text>'
            )

        # helper: log y position for ops/sec
        ops_log_min = math.log10(min(all_ops) * 0.8)
        ops_log_range = math.log10(ops_max) - ops_log_min
        mbs_log_min = math.log10(min(all_mbs) * 0.8)
        mbs_log_range = math.log10(mbs_max) - mbs_log_min

        def y_ops(val):
            return y_bot - (math.log10(val) - ops_log_min) / ops_log_range * panel_h

        def y_mbs(val):
            return y_bot - (math.log10(val) - mbs_log_min) / mbs_log_range * panel_h

        # right axis labels (throughput in MB/s or GB/s)
        log_mbs_min = math.floor(math.log10(min(all_mbs)))
        log_mbs_max = math.ceil(math.log10(mbs_max))
        for exp in range(log_mbs_min, log_mbs_max + 1):
            val = 10 ** exp
            if val > mbs_max:
                break
            yy = y_mbs(val)
            if yy < panel_top or yy > y_bot:
                continue
            if val >= 1e3:
                label = f"{val / 1e3:.0f} GB/s"
            elif val >= 100:
                label = f"{val:.0f} MB/s"
            elif val >= 10:
                label = f"{val:.0f} MB/s"
            else:
                label = f"{val:.0f} MB/s"
            L.append(
                f'  <text x="{margin_l + plot_w + 6}" y="{yy:.1f}" text-anchor="start"'
                f' dominant-baseline="middle" fill="#7d8590" font-size="9">{label}</text>'
            )

        # plot lines
        for codec in codecs:
            style = SWEEP_STYLES[codec]
            color = style["color"]
            rows = by_codec.get(codec, [])
            if not rows:
                continue

            pts_ops = []
            pts_mbs = []
            for r in rows:
                ns = get_ns(r)
                ops = 1e9 / ns
                mbs = r["input_size"] / ns * 1e3
                xx = x_pos(r["input_size"])
                pts_ops.append(f"{xx:.1f},{y_ops(ops):.1f}")
                pts_mbs.append(f"{xx:.1f},{y_mbs(mbs):.1f}")

            # ops/sec: dashed (thin)
            L.append(
                f'  <polyline points="{" ".join(pts_ops)}" fill="none"'
                f' stroke="{color}" stroke-width="1.2" stroke-dasharray="4,3"'
                f' stroke-opacity="0.55"/>'
            )
            # throughput: solid
            L.append(
                f'  <polyline points="{" ".join(pts_mbs)}" fill="none"'
                f' stroke="{color}" stroke-width="2"/>'
            )

            # dots on MB/s line
            for pt in pts_mbs:
                cx, cy = pt.split(",")
                L.append(
                    f'  <circle cx="{cx}" cy="{cy}" r="2" fill="{color}"/>'
                )

        return L

    L = []
    L.append(
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {svg_w} {svg_h}"'
        f' font-family="system-ui, -apple-system, sans-serif">'
    )
    L.append(f'  <rect width="{svg_w}" height="{svg_h}" fill="#0d1117"/>')

    # main title
    L.append(
        f'  <text x="{svg_w / 2}" y="22" text-anchor="middle" fill="#e6edf3"'
        f' font-size="14" font-weight="700">'
        f'LZ4 Size Sweep: Synthetic JSON (log-log)</text>'
    )
    if hw_label:
        L.append(
            f'  <text x="{svg_w / 2}" y="38" text-anchor="middle" fill="#7d8590"'
            f' font-size="10">{hw_label}</text>'
        )

    panel1_top = margin_top + 18
    panel2_top = panel1_top + panel_h + panel_gap

    p1 = make_panel(panel1_top, "Compress", lambda r: r["compress_ns"])
    if p1:
        L.extend(p1)
    p2 = make_panel(panel2_top, "Roundtrip (compress + decompress)",
                     lambda r: r["compress_ns"] + r["decompress_ns"])
    if p2:
        L.extend(p2)

    # x-axis label
    L.append(
        f'  <text x="{svg_w / 2}" y="{panel2_top + panel_h + 30}"'
        f' text-anchor="middle" fill="#e6edf3" font-size="11"'
        f' font-weight="600">input size</text>'
    )

    # legend: codecs (2x2 grid)
    leg_y = panel2_top + panel_h + 48
    leg_x_start = margin_l
    col_w = 160
    for i, codec in enumerate(codecs):
        style = SWEEP_STYLES[codec]
        col = i % 4
        row = i // 4
        lx = leg_x_start + col * col_w
        ly = leg_y + row * 20

        L.append(
            f'  <line x1="{lx}" y1="{ly}" x2="{lx + 20}" y2="{ly}"'
            f' stroke="{style["color"]}" stroke-width="2"/>'
        )
        L.append(
            f'  <text x="{lx + 26}" y="{ly + 4}" fill="#e6edf3"'
            f' font-size="10" font-weight="500">{SWEEP_LABELS[codec]}</text>'
        )

    # line style legend
    style_y = leg_y + 20
    L.append(
        f'  <line x1="{leg_x_start}" y1="{style_y}" x2="{leg_x_start + 20}" y2="{style_y}"'
        f' stroke="#7d8590" stroke-width="2"/>'
    )
    L.append(
        f'  <text x="{leg_x_start + 26}" y="{style_y + 4}" fill="#7d8590"'
        f' font-size="9">solid = throughput (right axis)</text>'
    )
    L.append(
        f'  <line x1="{leg_x_start + 220}" y1="{style_y}" x2="{leg_x_start + 240}" y2="{style_y}"'
        f' stroke="#7d8590" stroke-width="1.5" stroke-dasharray="4,3" stroke-opacity="0.6"/>'
    )
    L.append(
        f'  <text x="{leg_x_start + 246}" y="{style_y + 4}" fill="#7d8590"'
        f' font-size="9">thin = ops/sec (left axis)</text>'
    )

    L.append("</svg>")
    return "\n".join(L) + "\n"


def _hw_extras_available():
    if os.environ.get("LZ4RIP_HW_EXTRAS"):
        return True
    try:
        return open("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor").read().strip() == "performance"
    except OSError:
        return False


def generate_sweep_chart(out_dir):
    """Train dict, run sweep bench, generate chart."""
    if not _hw_extras_available():
        print("  error: LZ4RIP_HW_EXTRAS is not set and CPU governor is not 'performance'.", file=sys.stderr)
        print("  sweep.svg subtitle would be missing. Set the env var and retry:", file=sys.stderr)
        print("    LZ4RIP_HW_EXTRAS=\"performance governor,turbo off\" python3 -c ...", file=sys.stderr)
        return
    with tempfile.TemporaryDirectory() as tmp:
        sample_dir = Path(tmp) / "samples"
        sample_dir.mkdir()
        rng = random.Random(42)
        for i in range(DICT_TRAIN_COUNT):
            size = rng.randint(128, 2048)
            data = json_payload(size, counter_start=i * 100)
            (sample_dir / f"msg_{i:04d}.json").write_text(data)

        dict_path = Path(tmp) / "json.dict"
        if not train_dict(sample_dir, dict_path):
            print("  skipping sweep chart: dict training failed", file=sys.stderr)
            return

        cmd = ["cargo", "run", "--release", "--example", "lz4rip_bench", "--",
               "--sweep", str(dict_path)]
        has_taskset = shutil.which("taskset") is not None
        if has_taskset:
            cmd = ["taskset", "-c", "0"] + cmd
        print("  running sweep bench...", file=sys.stderr)
        result = subprocess.run(cmd, capture_output=True, text=True)
        if result.returncode != 0:
            print(f"  sweep bench failed: {result.stderr}", file=sys.stderr)
            return
        if result.stderr:
            print(result.stderr, end="", file=sys.stderr)

    import platform
    arch = platform.machine()
    results = load_cache_dir(cache_base() / arch / "sweep")
    if not results:
        print("  skipping sweep chart: no results in cache", file=sys.stderr)
        return

    svg = sweep_chart(results)
    if svg:
        out_path = out_dir / "sweep.svg"
        out_path.write_text(svg)
        print(f"  wrote {out_path}")


STRUCTURED_LABELS = {
    "C lz4":             "lz4 (C, stream)",
    "lz4rip":            "lz4rip (Compressor)",
    "lz4_flex unsafe":   "lz4_flex (unsafe, oneshot)",
    "lz4_flex":          "lz4_flex (safe, oneshot)",
}


def structured_chart(results, codec_order=None, colors=None, labels=None, title=None):
    """Pipeline bar chart for structured JSON, same style as pipeline_chart:
    stacked compress + transfer@1GB/s + decompress, seconds/GB, lower is better."""
    if codec_order is None:
        codec_order = CODEC_ORDER
    if colors is None:
        colors = COLORS
    if labels is None:
        labels = STRUCTURED_LABELS
    if title is None:
        title = "LZ4 Structured JSON: Compressor Reuse (256 B – 8 KB)"
    codecs = [c for c in codec_order if any(r["codec"] == c for r in results)]
    n_codecs = len(codecs)
    hw_label = detect_hardware()
    transfer_rate = 1e9

    sizes = sorted(set(r["input_size"] for r in results))
    n_sizes = len(sizes)

    stacks = {}
    y_max = 0
    for r in results:
        if r["codec"] not in codecs:
            continue
        per_gb = 1e9 / r["input_size"]
        comp = r["compress_ns"] / 1e9 * per_gb
        transfer = (r["compressed_size"] / r["input_size"]) * (1e9 / transfer_rate)
        decomp = r["decompress_ns"] / 1e9 * per_gb
        stacks[(r["input_size"], r["codec"])] = (comp, transfer, decomp)
        y_max = max(y_max, comp + transfer + decomp)

    y_max *= 1.1

    svg_w = 850
    x_left, x_right = 55, 830
    plot_w = x_right - x_left
    top_margin = 50 if hw_label else 40
    y_top = top_margin
    y_bot = top_margin + 340
    plot_h = y_bot - y_top

    svg_h = y_bot + 100

    def y(v):
        return y_bot - (v / y_max) * plot_h

    mid_x = (x_left + x_right) / 2
    L = []
    L.append(
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {svg_w} {svg_h}"'
        f' font-family="system-ui, -apple-system, sans-serif">'
    )
    L.append(f'  <rect width="{svg_w}" height="{svg_h}" fill="#0d1117"/>')

    L.append(
        f'  <text x="{mid_x}" y="22" text-anchor="middle" fill="#e6edf3"'
        f' font-size="14" font-weight="700">'
        f'{title}: Compress + Transfer @1 GB/s + Decompress (lower is better)'
        f'</text>'
    )
    if hw_label:
        L.append(
            f'  <text x="{mid_x}" y="38" text-anchor="middle" fill="#7d8590"'
            f' font-size="10">{hw_label}</text>'
        )

    # y gridlines
    step = nice_step(y_max, 5)
    v = step
    while v <= y_max:
        yy = y(v)
        L.append(
            f'  <line x1="{x_left}" y1="{yy:.1f}" x2="{x_right}" y2="{yy:.1f}"'
            f' stroke="#21262d" stroke-width="1"/>'
        )
        L.append(
            f'  <text x="{x_left - 8}" y="{yy:.1f}" text-anchor="end"'
            f' dominant-baseline="middle" fill="#7d8590" font-size="10">{v:.0f}</text>'
        )
        v += step

    # baseline
    L.append(
        f'  <line x1="{x_left}" y1="{y_bot}" x2="{x_right}" y2="{y_bot}"'
        f' stroke="#30363d" stroke-width="1.5"/>'
    )

    # y-axis label
    mid_y = (y_top + y_bot) / 2
    L.append(
        f'  <text x="22" y="{mid_y}" text-anchor="middle" fill="#e6edf3"'
        f' font-size="11" font-weight="600"'
        f' transform="rotate(-90,22,{mid_y})">seconds / GB</text>'
    )

    # bars: always lay out 4 slots so bar widths/positions match across charts
    SLOT_MAP = {
        "C lz4": 0, "lz4rip": 1, "lz4_flex unsafe": 2, "lz4_flex": 3,
        "C lz4 (dict 2K)": 0, "lz4rip (dict 2K)": 1,
    }
    n_slots = 4
    group_w = plot_w / n_sizes
    bar_w = group_w * 0.75 / n_slots
    gap = group_w * 0.25

    for gi, size in enumerate(sizes):
        group_x = x_left + gi * group_w + gap / 2

        for codec in codecs:
            if (size, codec) not in stacks:
                continue
            ci = SLOT_MAP.get(codec, 0)
            comp, transfer, decomp = stacks[(size, codec)]
            main_c, xfer_c = colors[codec]

            bx = group_x + ci * bar_w
            h_comp = (comp / y_max) * plot_h
            L.append(
                f'  <rect x="{bx:.1f}" y="{y(comp):.1f}"'
                f' width="{bar_w:.1f}" height="{h_comp:.1f}"'
                f' fill="{main_c}" rx="1"/>'
            )
            h_transfer = (transfer / y_max) * plot_h
            L.append(
                f'  <rect x="{bx:.1f}" y="{y(comp + transfer):.1f}"'
                f' width="{bar_w:.1f}" height="{h_transfer:.1f}"'
                f' fill="{xfer_c}" rx="1"/>'
            )
            h_decomp = (decomp / y_max) * plot_h
            L.append(
                f'  <rect x="{bx:.1f}" y="{y(comp + transfer + decomp):.1f}"'
                f' width="{bar_w:.1f}" height="{h_decomp:.1f}"'
                f' fill="{main_c}" rx="1"/>'
            )

        # size label
        cx = group_x + (n_slots * bar_w) / 2
        L.append(
            f'  <text x="{cx:.1f}" y="{y_bot + 16}" text-anchor="middle"'
            f' fill="#e6edf3" font-size="10" font-weight="600">{_fmt_size(size)}</text>'
        )

    # legend
    leg_y = y_bot + 35
    legend_items = [(k, labels.get(k, k)) for k in codecs]
    row_h = 18
    leg_positions = [(0, 0), (0, 1), (1, 0), (1, 1)]
    leg_col_x = [mid_x - 200, mid_x + 10]
    for i, (key, label) in enumerate(legend_items):
        if i >= len(leg_positions):
            break
        col, row = leg_positions[i]
        lx = leg_col_x[col]
        ly = leg_y + row * row_h
        main_c, _ = colors[key]
        L.append(
            f'  <rect x="{lx:.0f}" y="{ly - 5}" width="12" height="12"'
            f' fill="{main_c}" rx="2"/>'
        )
        L.append(
            f'  <text x="{lx + 18:.0f}" y="{ly + 5}" fill="#e6edf3"'
            f' font-size="10" font-weight="500">{label}</text>'
        )

    # bar segment legend
    n_legend_rows = 2
    seg_y = leg_y + n_legend_rows * row_h + 8
    seg_items = [
        ("bright = compress + decompress", "#e6edf3"),
        ("dim = transfer @1 GB/s", "#7d8590"),
    ]
    seg_total = 420
    seg_start = mid_x - seg_total / 2
    for i, (label, fill) in enumerate(seg_items):
        sx = seg_start + i * 240
        L.append(
            f'  <text x="{sx:.0f}" y="{seg_y + 4}" fill="{fill}"'
            f' font-size="9">{label}</text>'
        )

    L.append("</svg>")
    return "\n".join(L) + "\n"


def detect_arch():
    import platform
    return platform.machine()


def cache_base():
    home = Path(os.environ.get("HOME", "."))
    return home / ".cache" / "lz4rip"


def load_cache_dir(cache_dir):
    results = []
    if not cache_dir.is_dir():
        return results
    for f in sorted(cache_dir.glob("*.jsonl")):
        for line in f.read_text().splitlines():
            line = line.strip()
            if line:
                try:
                    results.append(json.loads(line))
                except json.JSONDecodeError:
                    pass
    return results


def filter_main_results(results):
    return [r for r in results if r["input"] in MAIN_INPUTS]


def main():
    if len(sys.argv) < 2:
        print(f"Usage: {sys.argv[0]} [--sweep|--structured|--all] <output_dir>",
              file=sys.stderr)
        print(f"  Reads cached results from ~/.cache/lz4rip/<arch>/", file=sys.stderr)
        sys.exit(1)

    arch = detect_arch()
    base = cache_base() / arch

    # --sweep mode: reads from cache/<arch>/sweep/
    if sys.argv[1] == "--sweep":
        out_dir = Path(sys.argv[2]) if len(sys.argv) > 2 else Path("doc/charts")
        out_dir.mkdir(exist_ok=True)
        results = load_cache_dir(base / "sweep")
        if not results:
            print("  no sweep results in cache", file=sys.stderr)
            return
        svg = sweep_chart(results)
        if svg:
            out_path = out_dir / "sweep.svg"
            out_path.write_text(svg)
            print(f"  wrote {out_path}")
        return

    # --structured mode: reads from cache/<arch>/structured/
    if sys.argv[1] == "--structured":
        out_dir = Path(sys.argv[2]) if len(sys.argv) > 2 else Path("doc/charts")
        out_dir.mkdir(parents=True, exist_ok=True)
        results = load_cache_dir(base / "structured")
        if not results:
            print("  no structured results in cache", file=sys.stderr)
            return
        no_dict = [r for r in results if "dict" not in r["codec"].lower()]
        if no_dict:
            svg = structured_chart(no_dict)
            if svg:
                out_path = out_dir / "no_dict.svg"
                out_path.write_text(svg)
                print(f"  wrote {out_path}")
        dict_results = [r for r in results if "dict" in r["codec"].lower()]
        if dict_results:
            svg = structured_chart(
                dict_results,
                codec_order=DICT_CODEC_ORDER,
                colors=DICT_COLORS,
                labels={"C lz4 (dict 2K)": "lz4 (C, dict)", "lz4rip (dict 2K)": "lz4rip (dict)"},
                title="LZ4 Structured JSON + Dict (2 KB)",
            )
            if svg:
                out_path = out_dir / "dict2k.svg"
                out_path.write_text(svg)
                print(f"  wrote {out_path}")
        return

    # --all mode: generate all charts from all cache subdirs
    if sys.argv[1] == "--all":
        out_dir = Path(sys.argv[2]) if len(sys.argv) > 2 else Path("doc/charts")
    else:
        out_dir = Path(sys.argv[1])
    out_dir.mkdir(exist_ok=True)

    # Main charts (pipeline + summary) from cache/<arch>/
    results = filter_main_results(load_cache_dir(base))
    if not results:
        print(f"  no main corpus results in {base}", file=sys.stderr)
        sys.exit(1)

    svg = pipeline_chart(results, out_dir)
    out_path = out_dir / "pipeline.svg"
    out_path.write_text(svg)
    print(f"  wrote {out_path}")

    svg = summary_chart(results, out_dir)
    out_path = out_dir / "summary.svg"
    out_path.write_text(svg)
    print(f"  wrote {out_path}")

    # Dict chart from main cache (dict codecs coexist there)
    generate_dict_charts(out_dir)

    # Sweep chart from cache/<arch>/sweep/
    sweep_results = load_cache_dir(base / "sweep")
    if sweep_results:
        svg = sweep_chart(sweep_results)
        if svg:
            out_path = out_dir / "sweep.svg"
            out_path.write_text(svg)
            print(f"  wrote {out_path}")

    # Structured charts from cache/<arch>/structured/
    struct_results = load_cache_dir(base / "structured")
    if struct_results:
        struct_dir = out_dir / "structured"
        struct_dir.mkdir(exist_ok=True)
        no_dict = [r for r in struct_results if "dict" not in r["codec"].lower()]
        if no_dict:
            svg = structured_chart(no_dict)
            if svg:
                out_path = struct_dir / "no_dict.svg"
                out_path.write_text(svg)
                print(f"  wrote {out_path}")
        dict_r = [r for r in struct_results if "dict" in r["codec"].lower()]
        if dict_r:
            svg = structured_chart(
                dict_r,
                codec_order=DICT_CODEC_ORDER,
                colors=DICT_COLORS,
                labels={"C lz4 (dict 2K)": "lz4 (C, dict)", "lz4rip (dict 2K)": "lz4rip (dict)"},
                title="LZ4 Structured JSON + Dict (2 KB)",
            )
            if svg:
                out_path = struct_dir / "dict2k.svg"
                out_path.write_text(svg)
                print(f"  wrote {out_path}")


if __name__ == "__main__":
    main()
