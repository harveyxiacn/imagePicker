"""Pixel helpers for masks: guided filter (He et al.), luminance, resizing, smoothstep.

All masks are float32 arrays in [0, 1]; images are RGB uint8. Only numpy + OpenCV are used
(`cv2.ximgproc` lives in opencv-contrib, which this project deliberately does not ship).
"""

from __future__ import annotations

import cv2
import numpy as np


def luminance(rgb: np.ndarray) -> np.ndarray:
    """Rec.601 luma of an RGB uint8 image as float32 in [0, 1]."""
    r = rgb[..., 0].astype(np.float32)
    g = rgb[..., 1].astype(np.float32)
    b = rgb[..., 2].astype(np.float32)
    return (0.299 * r + 0.587 * g + 0.114 * b) * (1.0 / 255.0)


def _box(x: np.ndarray, r: int) -> np.ndarray:
    return cv2.boxFilter(x, cv2.CV_32F, (2 * r + 1, 2 * r + 1), borderType=cv2.BORDER_REFLECT)


def guided_filter(guide: np.ndarray, src: np.ndarray, radius: int, eps: float) -> np.ndarray:
    """Edge-preserving smoothing of `src` steered by the single-channel `guide`.

    Both are float32 HxW (guide in [0, 1]). `radius` is the box half-size in pixels, `eps` the
    regularisation (variance scale): small eps = the output follows guide edges closely.
    The result is a locally linear function of the guide, so it snaps soft mask boundaries
    produced at low resolution to the edges of the full-resolution image.
    """
    guide = np.ascontiguousarray(guide, dtype=np.float32)
    src = np.ascontiguousarray(src, dtype=np.float32)
    r = max(1, int(radius))
    mean_i = _box(guide, r)
    mean_p = _box(src, r)
    cov_ip = _box(guide * src, r) - mean_i * mean_p
    var_i = _box(guide * guide, r) - mean_i * mean_i
    a = cov_ip / (var_i + eps)
    b = mean_p - a * mean_i
    return _box(a, r) * guide + _box(b, r)


def refine_alpha(
    alpha: np.ndarray, rgb: np.ndarray, radius_frac: float = 0.006, eps: float = 1e-3
) -> np.ndarray:
    """Guided-filter `alpha` (same HxW as `rgb`) against the image luminance; clipped to [0, 1]."""
    long_edge = max(rgb.shape[:2])
    radius = max(2, round(long_edge * radius_frac))
    out = guided_filter(luminance(rgb), alpha, radius, eps)
    return np.clip(out, 0.0, 1.0)


def smoothstep(x: np.ndarray | float, lo: float, hi: float) -> np.ndarray:
    t = np.clip((np.asarray(x, dtype=np.float32) - lo) / (hi - lo), 0.0, 1.0)
    return t * t * (3.0 - 2.0 * t)


def resize_mask(m: np.ndarray, width: int, height: int) -> np.ndarray:
    """Resize a float mask / probability map (area for shrinking, bilinear for growing)."""
    if m.shape[1] == width and m.shape[0] == height:
        return m
    shrink = width < m.shape[1] or height < m.shape[0]
    interp = cv2.INTER_AREA if shrink else cv2.INTER_LINEAR
    return cv2.resize(m, (width, height), interpolation=interp)


def to_u8(alpha: np.ndarray) -> np.ndarray:
    return np.clip(np.rint(alpha * 255.0), 0, 255).astype(np.uint8)
