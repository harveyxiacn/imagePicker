"""Latency of `beauty.prepare` (ms per face, with a per-stage breakdown).

Usage (from ai-worker/):
  uv run python scripts/bench_beauty.py <photo.jpg> [<photo2.jpg> ...] [--device auto|cpu]
                                         [--size 1536] [--runs 5] [--side-by-side]

All given photos are run as separate requests (faces detected by the worker, as for an empty
`faces` list); with `--side-by-side` the photos are first composed into one wide image so a single
request carries several people (per-face cost then includes the neighbour handling).
Timings are after warm-up (the first call loads models / initialises CUDA and is reported
separately). PNG writes are included. Missing models are downloaded unless --no-download.
"""

from __future__ import annotations

import argparse
import asyncio
import statistics
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import cv2  # noqa: E402
from PIL import Image  # noqa: E402

from imagepicker_ai import hw as hwmod  # noqa: E402
from imagepicker_ai.beauty import BeautyPreparer  # noqa: E402
from imagepicker_ai.decode import load_image  # noqa: E402
from imagepicker_ai.masks import MaskGenerator  # noqa: E402
from imagepicker_ai.models import ModelManager, Registry  # noqa: E402
from imagepicker_ai.paths import resolve_models_dir  # noqa: E402


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("photos", nargs="+")
    ap.add_argument("--device", default="auto")
    ap.add_argument("--size", type=int, default=1536)
    ap.add_argument("--runs", type=int, default=5)
    ap.add_argument("--models-dir")
    ap.add_argument("--side-by-side", action="store_true")
    ap.add_argument("--no-download", action="store_true")
    a = ap.parse_args()

    hw = hwmod.detect(a.device)
    mgr = ModelManager(resolve_models_dir(a.models_dir or ".models"), Registry.load(), hw)
    g = MaskGenerator(mgr, hw)
    bp = BeautyPreparer(g)
    if not a.no_download:
        for mid in bp.required_models():
            mgr.ensure(mid)

    tmp = Path(tempfile.mkdtemp(prefix="bench_beauty_"))
    photos = list(a.photos)
    if a.side_by_side and len(photos) > 1:
        ims = []
        for p in photos:
            im = load_image(p, 1400).rgb
            ims.append(cv2.resize(im, (round(im.shape[1] * 900 / im.shape[0]), 900)))
        out = tmp / "composite.jpg"
        Image.fromarray(cv2.hconcat(ims)).save(out, quality=95)
        photos = [str(out)]

    print(
        f"device={hw.device} providers={hw.providers[:1]} birefnet={g.subject_model()} size={a.size}"
    )
    print(
        f"{'photo':34s} {'faces':>5s} {'warmup':>8s} {'median':>8s} {'min':>6s} {'ms/face':>8s}  stages (ms/face)"
    )
    for p in photos:
        params = {
            "photo": {"photo_id": 1, "path": p},
            "size": a.size,
            "out_dir": str(tmp / "out"),
        }
        t0 = time.perf_counter()
        res = asyncio.run(bp.prepare(params))
        warm = (time.perf_counter() - t0) * 1000
        n = len(res["people"])
        if not n:
            print(f"{Path(p).name:34s} no faces ({res['skipped']})")
            continue
        walls, last = [], res
        for _ in range(a.runs):
            t0 = time.perf_counter()
            last = asyncio.run(bp.prepare(params))
            walls.append((time.perf_counter() - t0) * 1000)
        med = statistics.median(walls)
        stages = " ".join(
            f"{k[:-2]}={v * 1000 / n:.0f}"
            for k, v in last["timings"].items()
            if k != "wall_s" and v * 1000 / n >= 1
        )
        print(
            f"{Path(p).name[:34]:34s} {n:5d} {warm:8.0f} {med:8.0f} {min(walls):6.0f} {med / n:8.0f}  {stages}"
        )
        if last["skipped"]:
            print("  skipped:", last["skipped"])
    bp.shutdown()
    g.shutdown()


if __name__ == "__main__":
    main()
