"""Throughput benchmark for the analyze pipeline (images/s per step).

Usage (from ai-worker/):
  uv run python scripts/bench_analyze.py <photo-dir>            # real photos (jpg/png/heic...)
  uv run python scripts/bench_analyze.py --synth 200            # generated 12 MP JPEGs
  uv run python scripts/bench_analyze.py --synth 200 --device cpu
  uv run python scripts/bench_analyze.py --synth 200 --portrait .bench/img/Albert_Einstein_Head.jpg

Models must be installed (the script downloads missing ones unless --no-download).
Rows:
  decode        Pillow decode + EXIF + resize only (thread pool)
  <step>        full pipeline restricted to that single step (includes decode)
  embed_model   SigLIP2 forward only on pre-decoded 224px tensors (no decode)
  all           phash+quality+faces+embed together (decode overlapped with GPU)
"""

from __future__ import annotations

import argparse
import asyncio
import json
import sys
import tempfile
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import numpy as np
from PIL import Image

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from imagepicker_ai import hw as hwmod  # noqa: E402
from imagepicker_ai.decode import load_image  # noqa: E402
from imagepicker_ai.models import ModelManager, Registry  # noqa: E402
from imagepicker_ai.pipeline import Analyzer  # noqa: E402
from imagepicker_ai.steps.embed import prepare_image  # noqa: E402

EXTS = {".jpg", ".jpeg", ".png", ".heic", ".heif", ".webp", ".tif", ".tiff", ".bmp"}


def make_synth(n: int, out: Path, size=(4032, 3024), portrait: Path | None = None) -> list[Path]:
    import cv2

    out.mkdir(parents=True, exist_ok=True)
    paths: list[Path] = []
    rng = np.random.default_rng(0)
    base = []
    for _k in range(8):  # 8 distinct base scenes, jittered per image
        w, h = size
        low = rng.random((9, 12, 3)).astype(np.float32)
        img = cv2.resize(low, (w, h), interpolation=cv2.INTER_CUBIC)
        img = (img * 200 + 25).astype(np.uint8)
        for _ in range(60):
            c = tuple(int(v) for v in rng.integers(0, 255, 3))
            p = (int(rng.integers(0, w)), int(rng.integers(0, h)))
            if rng.random() < 0.5:
                cv2.circle(img, p, int(rng.integers(30, 300)), c, -1, cv2.LINE_AA)
            else:
                cv2.rectangle(
                    img,
                    p,
                    (p[0] + int(rng.integers(50, 600)), p[1] + int(rng.integers(50, 400))),
                    c,
                    -1,
                )
        base.append(img)
    for i in range(n):
        p = out / f"synth_{i:05d}.jpg"
        if not p.exists():
            if portrait is not None and i % 3 == 0:
                p.write_bytes(portrait.read_bytes())
            else:
                img = base[i % len(base)].astype(np.float32)
                img += np.random.default_rng(i).normal(0, 5, img.shape[:2])[..., None]
                Image.fromarray(img.clip(0, 255).astype(np.uint8)).save(p, "JPEG", quality=90)
        paths.append(p)
    return paths


def collect(directory: Path, limit: int | None) -> list[Path]:
    files = sorted(p for p in directory.rglob("*") if p.suffix.lower() in EXTS)
    return files[:limit] if limit else files


async def run(args: argparse.Namespace) -> dict:
    hw = hwmod.detect(args.device)
    models_dir = Path(args.models_dir)
    mgr = ModelManager(models_dir, Registry.load(), hw)
    emb_backend = "auto"
    an = Analyzer(
        mgr, hw, decode_workers=args.workers, batch_size=args.batch_size, embed_backend=emb_backend
    )

    need = ["yunet", "mediapipe-face-landmarker"] + an.embedder.required_models()
    for m in need:
        if not mgr.is_installed(m):
            if args.no_download:
                print(f"model {m} missing", file=sys.stderr)
                continue
            print(f"downloading {m} ...", file=sys.stderr)
            mgr.ensure(m)

    if args.synth:
        portrait = Path(args.portrait) if args.portrait else None
        t0 = time.time()
        files = make_synth(args.synth, Path(args.synth_dir), portrait=portrait)
        print(
            f"synthetic dataset ready ({len(files)} files, {time.time() - t0:.1f}s)",
            file=sys.stderr,
        )
    else:
        files = collect(Path(args.path), args.limit)
    if not files:
        raise SystemExit("no images found")

    items = [{"photo_id": i, "path": str(p)} for i, p in enumerate(files)]
    out_dir = tempfile.mkdtemp(prefix="ipbench_")
    n = len(items)
    rows: dict[str, dict] = {}

    # warm-up: model loads, CUDA context, thread pools
    await an.analyze_batch(
        {
            "items": items[: min(16, n)],
            "steps": ["phash", "quality", "faces", "embed"],
            "analysis_size": args.size,
            "out_dir": out_dir,
        }
    )

    # decode only
    t0 = time.perf_counter()
    with ThreadPoolExecutor(an.decode_workers) as pool:
        decoded = list(pool.map(lambda it: load_image(it["path"], args.size).rgb, items))
    dt = time.perf_counter() - t0
    rows["decode"] = {"images_per_s": n / dt, "wall_s": dt}

    for step in ("phash", "quality", "faces", "embed"):
        res = await an.analyze_batch(
            {"items": items, "steps": [step], "analysis_size": args.size, "out_dir": out_dir}
        )
        rows[step] = {
            "images_per_s": res["timings"]["images_per_s"],
            "wall_s": res["timings"]["wall_s"],
        }

    # model forward only
    prepared = [prepare_image(a) for a in decoded]
    an.embedder.embed_prepared(prepared[:8])
    t0 = time.perf_counter()
    an.embedder.embed_prepared(prepared)
    dt = time.perf_counter() - t0
    rows["embed_model"] = {"images_per_s": n / dt, "wall_s": dt}

    res = await an.analyze_batch(
        {
            "items": items,
            "steps": ["phash", "quality", "faces", "embed"],
            "analysis_size": args.size,
            "out_dir": out_dir,
        }
    )
    rows["all"] = {
        "images_per_s": res["timings"]["images_per_s"],
        "wall_s": res["timings"]["wall_s"],
        "cpu_s_per_step": {
            k: v for k, v in res["timings"].items() if k.endswith("_s") and k != "wall_s"
        },
    }
    info = {
        "device": res["device"],
        "tier": hw.tier,
        "gpu": hw.primary_gpu.name if hw.primary_gpu else None,
        "cpu": hw.cpu_name,
        "cores": hw.cpu_cores_logical,
        "decode_workers": an.decode_workers,
        "embed_model": res["models"].get("embed"),
        "faces_model": res["models"].get("faces"),
        "images": n,
        "analysis_size": args.size,
        "batch_size": an.embedder.batch_size,
    }
    an.shutdown()
    return {"info": info, "rows": rows}


def main() -> None:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument("path", nargs="?", help="directory of photos")
    ap.add_argument("--synth", type=int, default=0, help="generate N synthetic 12MP JPEGs instead")
    ap.add_argument("--synth-dir", default=".bench/synth")
    ap.add_argument(
        "--portrait", default=None, help="mix this real portrait in (every 3rd image) for face cost"
    )
    ap.add_argument("--limit", type=int, default=None)
    ap.add_argument("--device", choices=["auto", "cpu"], default="auto")
    ap.add_argument("--size", type=int, default=1024, help="analysis_size")
    ap.add_argument("--batch-size", type=int, default=32)
    ap.add_argument("--workers", type=int, default=None)
    ap.add_argument("--models-dir", default=".models")
    ap.add_argument("--no-download", action="store_true")
    ap.add_argument("--json", default=None, help="write results to this file")
    args = ap.parse_args()
    if not args.synth and not args.path:
        ap.error("give a directory or --synth N")

    result = asyncio.run(run(args))
    info, rows = result["info"], result["rows"]
    print(
        f"\n{info['gpu'] or info['cpu']} | device={info['device']['device']} tier={info['tier']} "
        f"| {info['images']} images @ {info['analysis_size']}px | decode workers={info['decode_workers']} "
        f"batch={info['batch_size']} | embed={info['embed_model']}"
    )
    print(f"{'step':<14}{'images/s':>10}{'wall s':>10}")
    for k in ("decode", "phash", "quality", "faces", "embed", "embed_model", "all"):
        r = rows[k]
        print(f"{k:<14}{r['images_per_s']:>10.1f}{r['wall_s']:>10.2f}")
    if args.json:
        Path(args.json).write_text(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
