"""Model-free sky mask: colour + brightness + position + gradient smoothness + connectivity.

Used when the `skyseg-u2net` weights are not installed (and as the reference in tests), so that
`mask.sky` always produces something sensible. Pipeline on a <= 384 px copy of the image:

  1. per-pixel candidate scores
       blue    hue 185-255 deg, some saturation, not dark           (clear sky)
       cloud   bright and nearly colourless                          (overcast / clouds)
       warm    orange-pink-red hue, bright, upper part of the frame  (sunrise / sunset)
  2. smoothness: skies have a low local gradient; cloud / warm candidates must be smooth
     (blue is trusted more, so cloud edges inside a blue sky do not punch holes)
  3. position prior: sky lives at the top; the weight decays towards the bottom
  4. hysteresis threshold (strong seeds + connected weaker pixels), morphological clean-up
  5. keep components that touch the top border or sit in the upper part, drop small ones
  6. optional exclusion of a foreground `subject` probability map

The caller upsamples the result and refines it with the guided filter.
"""

from __future__ import annotations

import cv2
import numpy as np

from .filters import smoothstep

WORK_EDGE = 384


def _hue_window(h: np.ndarray, lo: float, hi: float, soft: float) -> np.ndarray:
    """1 inside [lo, hi] degrees (circular), soft ramps of width `soft` outside."""
    mid = (lo + hi) / 2.0
    half = (hi - lo) / 2.0
    d = np.abs((h - mid + 180.0) % 360.0 - 180.0)
    return 1.0 - smoothstep(d, half, half + soft)


def sky_candidates(rgb: np.ndarray) -> tuple[np.ndarray, np.ndarray]:
    """(score, smooth) maps in [0, 1] for an RGB uint8 image (any size)."""
    f = rgb.astype(np.float32) * (1.0 / 255.0)
    hsv = cv2.cvtColor(f, cv2.COLOR_RGB2HSV)  # H in [0, 360)
    h, s, v = hsv[..., 0], hsv[..., 1], hsv[..., 2]
    hgt = rgb.shape[0]
    ys = (np.arange(hgt, dtype=np.float32) / max(hgt - 1, 1))[:, None]

    r, b = f[..., 0], f[..., 2]
    blue = (
        _hue_window(h, 190.0, 250.0, 20.0)
        * smoothstep(s, 0.07, 0.22)
        * smoothstep(v, 0.30, 0.50)
        * smoothstep(b - r, 0.0, 0.08)
    )
    cloud = smoothstep(v, 0.62, 0.80) * (1.0 - smoothstep(s, 0.08, 0.20))
    warm = (
        np.maximum(_hue_window(h, 345.0, 395.0, 20.0), _hue_window(h, 0.0, 50.0, 15.0))
        * smoothstep(s, 0.18, 0.40)
        * smoothstep(v, 0.45, 0.65)
        * (1.0 - smoothstep(ys, 0.35, 0.65))
    )

    gray = cv2.GaussianBlur(f.mean(axis=2), (0, 0), 1.2)
    gx = cv2.Sobel(gray, cv2.CV_32F, 1, 0, ksize=3)
    gy = cv2.Sobel(gray, cv2.CV_32F, 0, 1, ksize=3)
    grad = np.sqrt(gx * gx + gy * gy)
    grad = cv2.dilate(grad, np.ones((3, 3), np.uint8))
    smooth = 1.0 - smoothstep(grad, 0.05, 0.22)

    pos = 1.0 - smoothstep(ys, 0.45, 1.0)  # 1 at the top, 0 at the bottom
    blue_s = blue * (0.65 + 0.35 * smooth)
    cloud_s = 0.95 * cloud * smooth
    warm_s = 0.85 * warm * smooth
    score = np.maximum(blue_s, np.maximum(cloud_s, warm_s)) * (0.25 + 0.75 * pos)
    return score.astype(np.float32), smooth.astype(np.float32)


def sky_heuristic(rgb: np.ndarray, subject: np.ndarray | None = None) -> np.ndarray:
    """Sky probability map (float32 [0, 1], same HxW as `rgb`); `subject` = foreground prob. map."""
    h0, w0 = rgb.shape[:2]
    s = WORK_EDGE / max(h0, w0)
    if s < 1.0:
        small = cv2.resize(rgb, (max(1, round(w0 * s)), max(1, round(h0 * s))), cv2.INTER_AREA)
    else:
        small = rgb
    h, w = small.shape[:2]
    score, _ = sky_candidates(small)

    strong = (score > 0.45).astype(np.uint8)
    weak = (score > 0.22).astype(np.uint8)
    n, labels = cv2.connectedComponents(weak, connectivity=8)
    if n <= 1:
        return np.zeros((h0, w0), np.float32)
    seeded = np.zeros(n, bool)
    seeded[np.unique(labels[strong > 0])] = True
    seeded[0] = False
    region = seeded[labels].astype(np.uint8)

    k = max(3, round(max(h, w) / 90) | 1)
    kernel = cv2.getStructuringElement(cv2.MORPH_ELLIPSE, (k, k))
    region = cv2.morphologyEx(region, cv2.MORPH_OPEN, kernel)
    region = cv2.morphologyEx(region, cv2.MORPH_CLOSE, kernel)

    # keep sky-like components: touching the top border or centred in the upper part, not tiny
    n, labels, stats, cents = cv2.connectedComponentsWithStats(region, connectivity=8)
    keep = np.zeros(n, bool)
    top_rows = max(2, round(h * 0.03))
    for i in range(1, n):
        area = stats[i, cv2.CC_STAT_AREA]
        if area < 0.01 * h * w:
            continue
        touches_top = stats[i, cv2.CC_STAT_TOP] < top_rows
        upper = cents[i][1] < 0.45 * h
        if touches_top or upper:
            keep[i] = True
    kept = keep[labels].astype(np.float32)
    # fill small holes (clouds edges, sun) inside the sky
    inv = (kept < 0.5).astype(np.uint8)
    nh, lh, sh, _ = cv2.connectedComponentsWithStats(inv, connectivity=4)
    for i in range(1, nh):
        if sh[i, cv2.CC_STAT_AREA] < 0.004 * h * w:
            kept[lh == i] = 1.0

    kept = cv2.GaussianBlur(kept, (0, 0), 1.0)
    out = cv2.resize(kept, (w0, h0), interpolation=cv2.INTER_LINEAR)
    if subject is not None:
        out = out * (1.0 - np.clip(subject, 0.0, 1.0))
    return np.clip(out, 0.0, 1.0).astype(np.float32)
