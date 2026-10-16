"""Stage profiler of the analysis: where does the wait after an import go?

Two independent parts, each optional:

* worker (default): runs the AI worker's `analyze.batch` in-process on the photos of DIR, in
  requests of 32 like the Core, and records every step of every photo (decode, faces, identity,
  quality, phash, prep224, aesthetic, iqa) plus the embedding batches (SigLIP2 + zero-shot heads).
  Needs the worker's environment: `uv run --project ai-worker python bench/profile-analysis.py`.
* end to end (`--imagepicker BIN`): runs `BIN analyze DIR --export ... --timings-json ...` in a
  scratch data dir and reports import, the analysis stages (analysing, grouping, clustering,
  scoring) and the export. `--e2e-profile lite` runs without Python, models or a GPU.

Output: a JSON file (`--json`) and a markdown table (`--md`, also printed). `--dump-results`
writes the per-photo worker results (scores, tags, faces; paths removed) and `--compare A B`
diffs two such dumps, to show an optimisation left the results unchanged.

Examples (bench/README.md, "Analysis stage profile"):

    uv run --project ai-worker python bench/profile-analysis.py bench/data/photos200 \
        --profile standard --models-dir <models> --json bench/results/profile-gpu.json \
        --md bench/results/profile-gpu.md --imagepicker target/release/imagepicker
    python bench/profile-analysis.py --compare before.json after.json
"""

from __future__ import annotations

import argparse
import asyncio
import json
import math
import os
import platform
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import Any

IMAGE_EXT = {
    ".jpg", ".jpeg", ".png", ".tif", ".tiff", ".webp", ".heic", ".heif", ".avif",
    ".dng", ".cr2", ".cr3", ".nef", ".arw", ".raf", ".orf", ".rw2", ".pef", ".srw",
}  # fmt: skip
BATCH = 32  # photos per analyze.batch request (crates/ip-core/src/analysis/run.rs BATCH_SIZE)
STEP_ORDER = [
    "decode", "faces", "identity", "quality", "phash", "prep224", "aesthetic", "iqa",
    "embed (batched)", "heads (batched)",
]  # fmt: skip
# result fields that are paths or timings, not analysis output
VOLATILE = {"analysis_file", "embedding_file", "identity_file", "warning"}


def list_images(d: Path, limit: int | None) -> list[Path]:
    files = sorted(p for p in d.rglob("*") if p.is_file() and p.suffix.lower() in IMAGE_EXT)
    return files[:limit] if limit else files


def pct(xs: list[float], q: float) -> float:
    if not xs:
        return math.nan
    s = sorted(xs)
    k = (len(s) - 1) * q
    lo, hi = math.floor(k), math.ceil(k)
    return s[lo] + (s[hi] - s[lo]) * (k - lo)


def summarize(samples: dict[str, list[float]]) -> list[dict[str, Any]]:
    total_all = sum(sum(v) for v in samples.values()) or 1.0
    keys = [k for k in STEP_ORDER if k in samples] + sorted(set(samples) - set(STEP_ORDER))
    rows = []
    for k in keys:
        v = samples[k]
        rows.append(
            {
                "step": k,
                "photos": len(v),
                "total_s": round(sum(v), 3),
                "share": round(sum(v) / total_all, 4),
                "mean_ms": round(1000 * sum(v) / len(v), 2) if v else None,
                "p50_ms": round(1000 * pct(v, 0.5), 2) if v else None,
                "p95_ms": round(1000 * pct(v, 0.95), 2) if v else None,
            }
        )
    return rows


# ---------------------------------------------------------------------------------- worker part
def run_worker(args: argparse.Namespace, files: list[Path]) -> tuple[dict[str, Any], list[Any]]:
    from imagepicker_ai import hw as hwmod
    from imagepicker_ai.decode import load_image
    from imagepicker_ai.models.manager import ModelManager
    from imagepicker_ai.models.registry import Registry
    from imagepicker_ai.paths import resolve_models_dir
    from imagepicker_ai.pipeline import Analyzer

    hw = hwmod.detect(args.device)
    manager = ModelManager(resolve_models_dir(args.models_dir), Registry.load(), hw)
    an = Analyzer(manager, hw, decode_workers=args.decode_workers, batch_size=args.batch)
    if args.embed_flush:  # photos per GPU hand-off; the batch size = the pre-2026-10-16 behaviour
        an.embed_flush = args.embed_flush
    samples: dict[str, list[float]] = {}
    gpu_batches: list[dict[str, float]] = []

    def on_timing(kind: str, key: Any, tm: dict[str, float]) -> None:
        if kind == "item":
            for k, v in tm.items():
                samples.setdefault(k, []).append(v)
        else:  # one embedding batch: spread over its photos for the per-photo view
            gpu_batches.append({"photos": key, **tm})
            for k, v in tm.items():
                samples.setdefault(f"{k} (batched)", []).extend([v / key] * key)

    out_dir = Path(tempfile.mkdtemp(prefix="ip-profile-"))
    items = [{"photo_id": i, "path": str(p)} for i, p in enumerate(files)]
    base = {"analysis_size": args.analysis_size, "out_dir": str(out_dir)}
    if args.steps:
        base["steps"] = args.steps.split(",")
    else:
        base["profile"] = args.profile
    results: list[Any] = []
    meta: dict[str, Any] = {}

    async def go() -> float:
        # warm-up: loads the models (and the prompt cache, per-thread sessions) outside the
        # measured run; one photo per decode thread
        t0 = time.perf_counter()
        n_warm = min(len(items), max(4, an.decode_workers))
        warm = await an.analyze_batch({**base, "items": items[:n_warm]})
        meta.update(
            steps=warm["steps"],
            skipped_steps=warm["skipped_steps"],
            models=warm["models"],
            device=warm["device"],
            warnings=warm["warnings"],
        )
        warmup = time.perf_counter() - t0
        an.on_timing = on_timing
        for i in range(0, len(items), BATCH):
            r = await an.analyze_batch({**base, "items": items[i : i + BATCH]})
            results.extend(r["items"])
        an.on_timing = None
        return warmup

    try:
        t0 = time.perf_counter()
        warmup = asyncio.run(go())
        t_wall = time.perf_counter() - t0 - warmup
        if args.dump_results:  # the artefacts too: SigLIP2 and AuraFace vectors (float16 files)
            import numpy as np

            for r in results:
                for key, out in (("embedding_file", "embedding"), ("identity_file", "identity")):
                    if r and r.get(key):
                        r[out] = np.load(r[key]).astype(float).round(6).tolist()
    finally:
        an.shutdown()
        shutil.rmtree(out_dir, ignore_errors=True)

    # decode alone, one thread, on a sample: the cost without contention from the other steps
    iso: list[float] = []
    for p in files[: args.decode_sample]:
        t = time.perf_counter()
        try:
            load_image(str(p), args.analysis_size)
        except Exception:  # noqa: BLE001
            continue
        iso.append(time.perf_counter() - t)

    ok = sum(1 for r in results if r and "error" not in r)
    errors: dict[str, int] = {}
    for r in results:
        if r and "error" in r:
            k = r["error"].get("kind", "?")
            errors[k] = errors.get(k, 0) + 1
    report = {
        **meta,
        "profile": None if args.steps else args.profile,
        "photos": len(files),
        "ok": ok,
        "errors": errors,
        "decode_workers": an.decode_workers,
        "batch_size": args.batch,
        "embed_flush": getattr(an, "embed_flush", None),
        "analysis_size": args.analysis_size,
        "warmup_s": round(warmup, 3),
        "wall_s": round(t_wall, 3),
        "photos_per_s": round(ok / t_wall, 2) if t_wall > 0 else None,
        "ms_per_photo_wall": round(1000 * t_wall / max(1, len(files)), 2),
        "steps_per_photo": summarize(samples),
        "gpu_batches": len(gpu_batches),
        "decode_isolated": {
            "photos": len(iso),
            "p50_ms": round(1000 * pct(iso, 0.5), 2) if iso else None,
            "p95_ms": round(1000 * pct(iso, 0.95), 2) if iso else None,
        },
    }
    return report, results


# ------------------------------------------------------------------------------- end-to-end part
def run_e2e(args: argparse.Namespace, src: Path) -> dict[str, Any]:
    work = Path(tempfile.mkdtemp(prefix="ip-profile-e2e-"))
    tj = work / "timings.json"
    cmd = [
        args.imagepicker, "analyze", str(src), "--profile", args.e2e_profile,
        "--data-dir", str(work / "data"), "--face-consent",
        "--export", str(work / "export"), "--timings-json", str(tj),
    ]  # fmt: skip
    if args.models_dir:
        cmd += ["--models-dir", args.models_dir]
    t0 = time.perf_counter()
    try:
        proc = subprocess.run(cmd, capture_output=True, text=True, timeout=args.e2e_timeout)
        wall = time.perf_counter() - t0
        if proc.returncode != 0 or not tj.is_file():
            raise SystemExit(
                f"imagepicker analyze failed ({proc.returncode}):\n{proc.stdout[-2000:]}\n"
                f"{proc.stderr[-2000:]}"
            )
        t = json.loads(tj.read_text("utf-8"))
    finally:
        if not args.keep:
            shutil.rmtree(work, ignore_errors=True)
    n = max(1, t.get("photos") or 0)
    stages = [{"stage": "import", "s": t.get("import_s")}]
    stages += t.get("stages", [])
    stages.append({"stage": "export", "s": t.get("export_s")})
    for s in stages:
        s["ms_per_photo"] = round(1000 * s["s"] / n, 2) if s.get("s") is not None else None
        s["s"] = round(s["s"], 3) if s.get("s") is not None else None
    return {**t, "process_wall_s": round(wall, 3), "stages": stages}


# ------------------------------------------------------------------------------------- reports
def fmt(v: Any) -> str:
    if v is None or (isinstance(v, float) and math.isnan(v)):
        return "-"
    return str(v)


def markdown(rep: dict[str, Any]) -> str:
    m = rep["machine"]
    lines = [
        f"## Analysis profile: {rep.get('label') or ''} ({rep['photos']} photos from `{rep['dir']}`)",
        "",
        f"Machine: {m['node']} / {m['system']} / {m['cpu']} ({m['cores']} logical cores), "
        f"python {m['python']}. Date {rep['date']}.",
        "",
    ]
    w = rep.get("worker")
    if w:
        lines += [
            f"### Worker `analyze.batch`, profile `{w['profile'] or 'custom'}`",
            "",
            f"Steps run: {', '.join(w['steps'])}; skipped: {', '.join(w['skipped_steps']) or 'none'}. "
            f"Device: {w['device']['device']} ({', '.join(w['device']['providers'] or [])}). "
            f"Decode threads {w['decode_workers']}, batch {w['batch_size']} (GPU hand-off every "
            f"{w.get('embed_flush')}), analysis size {w['analysis_size']}.",
            "",
            f"**{w['ok']} photos in {w['wall_s']} s = {w['photos_per_s']} photos/s** "
            f"({w['ms_per_photo_wall']} ms per photo wall; model warm-up {w['warmup_s']} s not included"
            + (f"; errors {w['errors']}" if w["errors"] else "")
            + ").",
            "",
            "| step | photos | total s | share | mean ms | p50 ms | p95 ms |",
            "|---|---:|---:|---:|---:|---:|---:|",
        ]
        for r in w["steps_per_photo"]:
            lines.append(
                f"| {r['step']} | {r['photos']} | {r['total_s']} | {r['share']:.1%} | "
                f"{fmt(r['mean_ms'])} | {fmt(r['p50_ms'])} | {fmt(r['p95_ms'])} |"
            )
        di = w["decode_isolated"]
        lines += [
            "",
            "Step times are per photo on the decode threads (they run in parallel, so the sum "
            "exceeds the wall time); batched steps are the batch time divided by its photos. "
            f"Decode alone on one thread: p50 {fmt(di['p50_ms'])} ms, p95 {fmt(di['p95_ms'])} ms "
            f"({di['photos']} photos).",
            "",
        ]
    e = rep.get("e2e")
    if e:
        lines += [
            f"### End to end (`imagepicker analyze --profile {e['profile']}` + export)",
            "",
            f"{e['analyzed']} of {e['photos']} photos analysed, state {e['state']}, "
            f"{e.get('exported')} exported; process wall {e['process_wall_s']} s.",
            "",
            "| stage | s | ms per photo |",
            "|---|---:|---:|",
        ]
        for s in e["stages"]:
            lines.append(f"| {s['stage']} | {fmt(s['s'])} | {fmt(s['ms_per_photo'])} |")
        lines.append("")
    return "\n".join(lines)


def machine() -> dict[str, Any]:
    cpu = platform.processor() or platform.machine()
    if sys.platform == "win32":
        try:
            import winreg

            k = winreg.OpenKey(
                winreg.HKEY_LOCAL_MACHINE, r"HARDWARE\DESCRIPTION\System\CentralProcessor\0"
            )
            cpu = winreg.QueryValueEx(k, "ProcessorNameString")[0].strip()
        except OSError:
            pass
    return {
        "node": platform.node(),
        "system": f"{platform.system()} {platform.release()}",
        "cpu": cpu,
        "cores": os.cpu_count(),
        "python": platform.python_version(),
    }


# ------------------------------------------------------------------------------------- compare
def _norm(v: Any) -> Any:
    if isinstance(v, dict):
        return {k: _norm(x) for k, x in v.items() if k not in VOLATILE}
    if isinstance(v, list):
        return [_norm(x) for x in v]
    return v


def diff(a: Any, b: Any, tol: float, path: str = "") -> list[str]:
    if isinstance(a, dict) and isinstance(b, dict):
        out = []
        for k in sorted(set(a) | set(b)):
            if k not in a or k not in b:
                out.append(f"{path}.{k}: only in {'after' if k in b else 'before'}")
            else:
                out += diff(a[k], b[k], tol, f"{path}.{k}")
        return out
    if isinstance(a, list) and isinstance(b, list):
        if len(a) != len(b):
            return [f"{path}: {len(a)} vs {len(b)} entries"]
        return [
            d
            for i, (x, y) in enumerate(zip(a, b, strict=True))
            for d in diff(x, y, tol, f"{path}[{i}]")
        ]
    if isinstance(a, (int, float)) and isinstance(b, (int, float)) and not isinstance(a, bool):
        return [] if abs(a - b) <= tol else [f"{path}: {a} vs {b}"]
    return [] if a == b else [f"{path}: {a!r} vs {b!r}"]


def compare(a_path: str, b_path: str, tol: float) -> int:
    a = {str(r["photo_id"]): _norm(r) for r in json.loads(Path(a_path).read_text("utf-8"))}
    b = {str(r["photo_id"]): _norm(r) for r in json.loads(Path(b_path).read_text("utf-8"))}
    problems = []
    for pid in sorted(set(a) | set(b)):
        if pid not in a or pid not in b:
            problems.append(f"photo {pid}: missing on one side")
            continue
        problems += [f"photo {pid}{d}" for d in diff(a[pid], b[pid], tol)]
    for p in problems[:50]:
        print(p)
    print(f"{len(a)} vs {len(b)} photos, {len(problems)} differences (tolerance {tol})")
    return 1 if problems else 0


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument("dir", nargs="?", help="folder of photos (searched recursively)")
    ap.add_argument("--profile", default="standard", help="worker profile: fast or standard")
    ap.add_argument("--steps", help="explicit comma-separated worker steps instead of a profile")
    ap.add_argument("--limit", type=int, help="first N photos only (sorted by path)")
    ap.add_argument(
        "--models-dir", help="worker models (default: IMAGEPICKER_MODELS_DIR / app data)"
    )
    ap.add_argument("--device", default="auto", help="auto, cpu, cuda, directml, coreml")
    ap.add_argument(
        "--decode-workers", type=int, help="worker decode threads (default: cores-1, 2..8)"
    )
    ap.add_argument("--batch", type=int, default=32, help="embedding batch size")
    ap.add_argument(
        "--embed-flush", type=int, help="photos per GPU hand-off (worker default 8; 32 = old)"
    )
    ap.add_argument("--analysis-size", type=int, default=1024)
    ap.add_argument(
        "--decode-sample", type=int, default=50, help="photos for the isolated decode timing"
    )
    ap.add_argument("--no-worker", action="store_true", help="skip the in-process worker part")
    ap.add_argument(
        "--imagepicker", help="imagepicker binary: also run import -> analysis -> export"
    )
    ap.add_argument("--e2e-profile", default="standard", help="profile of the end-to-end run")
    ap.add_argument("--e2e-timeout", type=float, default=7200)
    ap.add_argument("--keep", action="store_true", help="keep the end-to-end scratch dir")
    ap.add_argument("--label", default="")
    ap.add_argument("--json", help="write the report as JSON")
    ap.add_argument("--md", help="write the markdown table")
    ap.add_argument("--dump-results", help="write the per-photo worker results (for --compare)")
    ap.add_argument("--compare", nargs=2, metavar=("BEFORE", "AFTER"), help="diff two result dumps")
    ap.add_argument("--tol", type=float, default=0.0, help="float tolerance of --compare")
    args = ap.parse_args()

    if args.compare:
        return compare(*args.compare, args.tol)
    if not args.dir:
        ap.error("dir is required")
    src = Path(args.dir)
    files = list_images(src, args.limit)
    if not files:
        ap.error(f"no images under {src}")
    rep: dict[str, Any] = {
        "label": args.label,
        "dir": str(src),
        "photos": len(files),
        "date": time.strftime("%Y-%m-%d %H:%M"),
        "machine": machine(),
    }
    if not args.no_worker:
        rep["worker"], results = run_worker(args, files)
        if args.dump_results:
            for r in results:
                if r:
                    r["file"] = files[int(r["photo_id"])].name
            Path(args.dump_results).write_text(json.dumps(results, indent=1), "utf-8")
    if args.imagepicker:
        e2e_src = src
        if args.limit:  # the CLI imports a folder: hand it exactly the profiled photos
            e2e_src = Path(tempfile.mkdtemp(prefix="ip-profile-src-"))
            for i, f in enumerate(files):
                shutil.copy2(f, e2e_src / f"{i:05d}_{f.name}")
        try:
            rep["e2e"] = run_e2e(args, e2e_src)
        finally:
            if e2e_src != src:
                shutil.rmtree(e2e_src, ignore_errors=True)
    md = markdown(rep)
    print(md)
    if args.json:
        Path(args.json).write_text(json.dumps(rep, indent=1), "utf-8")
    if args.md:
        Path(args.md).write_text(md, "utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main())
