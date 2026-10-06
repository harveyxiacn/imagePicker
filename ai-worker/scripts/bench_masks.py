"""Latency of `mask.generate` targets (ms per target, model forward + refinement, PNG write excluded).

Usage (from ai-worker/):
  uv run python scripts/bench_masks.py <photo.jpg> [--device auto|cpu] [--size 1024] [--runs 5]
  uv run python scripts/bench_masks.py <photo.jpg> --device cpu --subject-model birefnet-lite-512

Each timed run uses a fresh photo state (no cache shared between targets), so the numbers are what
a request with that single target costs after the models are loaded; `warmup` is the first call
(model load + CUDA init). Missing models are downloaded unless --no-download. `person` uses the
largest detected face as person_bbox.
"""

from __future__ import annotations

import argparse
import statistics
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import cv2  # noqa: E402

from imagepicker_ai import hw as hwmod  # noqa: E402
from imagepicker_ai.decode import load_image  # noqa: E402
from imagepicker_ai.masks.generator import TARGETS, MaskGenerator, _Photo  # noqa: E402
from imagepicker_ai.models import ModelManager, Registry  # noqa: E402
from imagepicker_ai.paths import resolve_models_dir  # noqa: E402


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("photo")
    ap.add_argument("--device", default="auto")
    ap.add_argument("--size", type=int, default=1024)
    ap.add_argument("--runs", type=int, default=5)
    ap.add_argument("--models-dir")
    ap.add_argument("--subject-model", help="force a BiRefNet registry id")
    ap.add_argument("--no-download", action="store_true")
    a = ap.parse_args()

    hw = hwmod.detect(a.device)
    reg = Registry.load()
    mgr = ModelManager(resolve_models_dir(a.models_dir or ".models"), reg, hw)
    g = MaskGenerator(mgr, hw)
    if a.subject_model:
        g.subject_model = lambda: a.subject_model  # type: ignore[method-assign]
    if not a.no_download:
        for mid in g.models_for_targets(list(TARGETS)):
            mgr.ensure(mid)

    rgb = load_image(a.photo, a.size).rgb
    s = a.size / max(rgb.shape[:2])
    if abs(s - 1) > 1e-3:
        rgb = cv2.resize(rgb, (round(rgb.shape[1] * s), round(rgb.shape[0] * s)))
    print(f"device={hw.device} providers={hw.providers[:1]} subject_model={g.subject_model()}")
    print(f"image {rgb.shape[1]}x{rgb.shape[0]} (size={a.size})")
    faces = g._detect_faces(_Photo(rgb))
    bbox = None
    if faces:
        x, y, w, h = max(faces, key=lambda f: f[2] * f[3])
        bbox = [x / rgb.shape[1], y / rgb.shape[0], w / rgb.shape[1], h / rgb.shape[0]]
    print(f"{'target':10s} {'model':28s} {'warmup':>9s} {'median':>9s} {'min':>9s}  (ms)")
    for t in TARGETS:
        try:
            t0 = time.perf_counter()
            _, model = g._run_target(_Photo(rgb), t, bbox)
            warm = (time.perf_counter() - t0) * 1000
            ts = []
            for _ in range(a.runs):
                t0 = time.perf_counter()
                g._run_target(_Photo(rgb), t, bbox)
                ts.append((time.perf_counter() - t0) * 1000)
            print(f"{t:10s} {model:28s} {warm:9.0f} {statistics.median(ts):9.0f} {min(ts):9.0f}")
        except Exception as e:  # noqa: BLE001
            print(f"{t:10s} FAILED: {e}")
    g.shutdown()


if __name__ == "__main__":
    main()
