"""Regenerates the cross-check fixtures of `crates/ip-lite` (Rust port of phash + quality).

    cd ai-worker && uv run python scripts/gen_lite_fixtures.py

Writes deterministic synthetic PNGs plus `expected.json` (the worker's phash / quality output for
each) into `crates/ip-lite/tests/fixtures/`. `crates/ip-lite/tests/parity.rs` checks the Rust
implementation against them (pHash bit-exact, quality within float32 tolerance).
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

import cv2
import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from imagepicker_ai.steps.phash import phash_hex  # noqa: E402
from imagepicker_ai.steps.quality import analyze_quality  # noqa: E402

OUT = Path(__file__).resolve().parents[2] / "crates" / "ip-lite" / "tests" / "fixtures"


def coords(w: int, h: int) -> tuple[np.ndarray, np.ndarray]:
    y, x = np.mgrid[0:h, 0:w].astype(np.float32)
    return x / max(w - 1, 1), y / max(h - 1, 1)


def scene(w: int, h: int, seed: int) -> np.ndarray:
    """Smooth colour field with a few hard-edged shapes (photo-like statistics)."""
    rng = np.random.default_rng(seed)
    x, y = coords(w, h)
    img = np.zeros((h, w, 3), np.float32)
    for c in range(3):
        f = rng.uniform(1.0, 4.0, 2)
        p = rng.uniform(0, 6.28, 2)
        img[..., c] = 128 + 90 * np.sin(f[0] * 6.28 * x + p[0]) * np.cos(f[1] * 6.28 * y + p[1])
    for _ in range(6):
        cx, cy = int(rng.uniform(0.1, 0.9) * w), int(rng.uniform(0.1, 0.9) * h)
        r = int(rng.uniform(0.05, 0.2) * min(w, h))
        col = tuple(int(v) for v in rng.integers(20, 235, 3))
        cv2.circle(img, (cx, cy), r, col, -1)
        cv2.rectangle(img, (cx - r, cy + r // 2), (cx + r, cy + r), col[::-1], -1)
    return np.clip(img, 0, 255).astype(np.uint8)


def noisy(base: np.ndarray, sigma: float, seed: int) -> np.ndarray:
    rng = np.random.default_rng(seed)
    n = rng.normal(0, sigma, base.shape)
    return np.clip(base.astype(np.float32) + n, 0, 255).astype(np.uint8)


def checker(w: int, h: int, cell: int, ramp: float = 0.0) -> np.ndarray:
    yy, xx = np.mgrid[0:h, 0:w]
    v = np.where(((xx // cell) + (yy // cell)) % 2 == 0, 180, 40).astype(np.float32)
    v = np.clip(v + ramp * xx / max(w - 1, 1), 0, 255).astype(np.uint8)
    return np.dstack([v, v, v])


def build() -> dict[str, np.ndarray]:
    s1024 = scene(1024, 768, 1)
    return {
        "scene_1024x768": s1024,
        "scene_1024x768_blur": cv2.GaussianBlur(s1024, (0, 0), 3.0),
        "scene_1024x768_noise": noisy(s1024, 14.0, 7),
        "scene_333x250": scene(333, 250, 2),  # non-integer area ratio to 32x32
        "scene_700x500": scene(700, 500, 3),  # quality: bilinear up to 1024
        "scene_1500x1000": scene(1500, 1000, 4),  # quality: area down to 1024
        "scene_512x512": scene(512, 512, 5),  # integer ratio to 32x32
        "checker_256x192": noisy(checker(256, 192, 13, 70.0), 5.0, 21),  # noise: no exact-zero DCT terms
        "dark_640x480": (scene(640, 480, 6) * 0.15).astype(np.uint8),
        "bright_640x480": np.clip(scene(640, 480, 8).astype(np.int32) + 120, 0, 255).astype(np.uint8),
        "scene_100x75": scene(100, 75, 9),
        "scene_40x30": scene(40, 30, 10),
        "scene_20x20": scene(20, 20, 11),  # smaller than 32: pHash enlarges (bilinear)
        "portrait_768x1024": scene(768, 1024, 12),
        "noise_only_480x360": np.random.default_rng(13).integers(0, 256, (360, 480, 3), dtype=np.uint8),
    }


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    expected: dict[str, dict] = {}
    for name, rgb in build().items():
        cv2.imwrite(str(OUT / f"{name}.png"), cv2.cvtColor(rgb, cv2.COLOR_RGB2BGR))
        expected[name] = {
            "width": rgb.shape[1],
            "height": rgb.shape[0],
            "phash": phash_hex(rgb),
            "quality": analyze_quality(rgb),
        }
    (OUT / "expected.json").write_text(json.dumps(expected, indent=1, sort_keys=True) + "\n")
    print(f"wrote {len(expected)} fixtures to {OUT}")


if __name__ == "__main__":
    main()
