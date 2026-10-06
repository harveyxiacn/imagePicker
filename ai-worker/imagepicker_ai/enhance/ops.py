"""Pixel-level implementations of `enhance.run` operations (models injected as callables).

denoise       tiled SCUNet, feathered overlap blend, strength -> linear blend with the input
upscale       tiled Real-ESRGAN (context discarded around each tile), strength -> blend with bicubic
face_restore  GFPGAN on FFHQ-aligned 512 crops, pasted back with a feathered oval mask
"""

from __future__ import annotations

import math
from collections.abc import Callable
from dataclasses import dataclass

import cv2
import numpy as np

from ..beauty import landmarks as lmk
from ..facemesh import FaceObs
from ..tiling import run_tiled

# FFHQ 5-point template at 512 px (facexlib / GFPGAN): eye L, eye R, nose, mouth L, mouth R
FFHQ_512 = np.array(
    [
        [192.98138, 239.94708],
        [318.90277, 240.1936],
        [256.63416, 314.01935],
        [201.26117, 371.41043],
        [313.08905, 371.15118],
    ],
    np.float64,
)
MIN_FACE_PX = 48  # smaller faces carry no detail to restore: restoring would only invent one
SIZE = 512


def estimate_noise(rgb: np.ndarray) -> float:
    """Noise sigma (0-255 scale) from the MAD of the luminance high-pass, edges excluded."""
    g = cv2.cvtColor(rgb, cv2.COLOR_RGB2GRAY).astype(np.float32)
    if max(g.shape) > 1500:
        s = 1500 / max(g.shape)
        g = cv2.resize(g, None, fx=s, fy=s, interpolation=cv2.INTER_AREA)
    hp = g - cv2.GaussianBlur(g, (0, 0), 1.2)
    grad = cv2.GaussianBlur(np.hypot(*np.gradient(g)), (0, 0), 1.5)
    flat = grad < np.percentile(grad, 40)
    v = hp[flat]
    return float(np.median(np.abs(v - np.median(v))) * 1.4826) if v.size > 100 else 0.0


def auto_strength(rgb: np.ndarray) -> float:
    """ISO-adaptive default: sigma ~1 -> 0.2 (clean), sigma >= 6 -> 1.0."""
    return float(np.clip((estimate_noise(rgb) - 0.5) / 5.5, 0.2, 1.0))


def blend_u8(a: np.ndarray, b: np.ndarray, s: float) -> np.ndarray:
    """a + s * (b - a) in uint8."""
    if s >= 0.999:
        return b
    out = a.astype(np.float32)
    out += (b.astype(np.float32) - out) * s
    return np.clip(np.rint(out), 0, 255).astype(np.uint8)


def denoise_tiled(
    rgb: np.ndarray,
    net: Callable[[np.ndarray], np.ndarray],
    tile: int,
    strength: float,
    progress: Callable[[int, int], None] | None = None,
) -> np.ndarray:
    den = run_tiled(
        rgb, net, tile=tile, overlap=32, scale=1, multiple=8, blend=True, progress=progress
    )
    return blend_u8(rgb, den, strength)


def upscale_tiled(
    rgb: np.ndarray,
    net: Callable[[np.ndarray], np.ndarray],
    scale: int,
    tile: int,
    strength: float = 1.0,
    progress: Callable[[int, int], None] | None = None,
) -> np.ndarray:
    up = run_tiled(
        rgb, net, tile=tile, overlap=16, scale=scale, multiple=1, blend=False, progress=progress
    )
    if strength < 0.999:
        h, w = rgb.shape[:2]
        base = cv2.resize(rgb, (w * scale, h * scale), interpolation=cv2.INTER_CUBIC)
        return blend_u8(base, up, strength)
    return up


# ---------------------------------------------------------------------------------- face restore
def five_points(pts: np.ndarray) -> np.ndarray:
    """(5, 2) eye L, eye R, nose, mouth L, mouth R (image-left first) from the 478 landmarks."""
    return np.array([pts[468], pts[473], pts[1], pts[61], pts[291]], np.float64)


@dataclass
class RestoredFace:
    rgba: np.ndarray  # HxWx4 patch
    rect: tuple[int, int, int, int]  # x, y, w, h in photo pixels


def restore_face(
    rgb: np.ndarray,
    obs: FaceObs,
    gfp: Callable[[np.ndarray], np.ndarray],
    strength: float,
) -> RestoredFace | None:
    """`gfp`: [1,3,512,512] float in -1..1 -> same. None when the face is too small to restore."""
    h, w = rgb.shape[:2]
    pts = obs.pts
    fw = obs.width
    if fw < MIN_FACE_PX:
        return None
    M, _ = cv2.estimateAffinePartial2D(five_points(pts), FFHQ_512, method=cv2.LMEDS)
    if M is None:
        return None
    s_m = math.sqrt(abs(M[0, 0] * M[1, 1] - M[0, 1] * M[1, 0]))  # photo px -> aligned px
    Mi = cv2.invertAffineTransform(M)
    corners = (
        np.array([[0, 0], [SIZE, 0], [SIZE, SIZE], [0, SIZE]], np.float64) @ Mi[:, :2].T + Mi[:, 2]
    )
    x0 = max(0, int(math.floor(corners[:, 0].min())) - 4)
    y0 = max(0, int(math.floor(corners[:, 1].min())) - 4)
    x1 = min(w, int(math.ceil(corners[:, 0].max())) + 4)
    y1 = min(h, int(math.ceil(corners[:, 1].max())) + 4)
    crop = np.ascontiguousarray(rgb[y0:y1, x0:x1])
    ch, cw = crop.shape[:2]
    # face crop -> aligned 512; shrink first when the face is large (no aliasing)
    k = min(1.0, s_m)
    Mc = M.copy()
    Mc[:, 2] += Mc[:, :2] @ np.array([x0, y0])  # crop coordinates -> aligned
    if k < 1.0:
        small = cv2.resize(crop, None, fx=k, fy=k, interpolation=cv2.INTER_AREA)
        S = np.array([[k, 0, 0], [0, k, 0], [0, 0, 1.0]])
        Mk = (np.vstack([Mc, [0, 0, 1]]) @ np.linalg.inv(S))[:2]
        aligned = cv2.warpAffine(
            small, Mk, (SIZE, SIZE), flags=cv2.INTER_LINEAR, borderMode=cv2.BORDER_REFLECT_101
        )
    else:
        aligned = cv2.warpAffine(
            crop, Mc, (SIZE, SIZE), flags=cv2.INTER_CUBIC, borderMode=cv2.BORDER_REFLECT_101
        )
    x = aligned.astype(np.float32) * (1 / 127.5) - 1.0
    y = np.asarray(gfp(np.ascontiguousarray(x.transpose(2, 0, 1)[None])), np.float32)[0]
    restored = np.clip(np.rint((y.transpose(1, 2, 0) + 1.0) * 127.5), 0, 255).astype(np.uint8)
    # back into the photo's crop (inverse of the alignment, at the photo's resolution)
    Mc_inv = cv2.invertAffineTransform(Mc)
    back = cv2.warpAffine(
        restored, Mc_inv, (cw, ch), flags=cv2.INTER_CUBIC, borderMode=cv2.BORDER_REPLICATE
    )
    if s_m < 0.999:
        # large face: GFPGAN works at 512 px, below the photo's own detail. Take its low
        # frequencies only (colour, shape, skin tone) and keep the photo's detail band.
        sig = max(0.6, 0.5 / s_m)
        orig = crop.astype(np.float32)
        back = np.clip(
            cv2.GaussianBlur(back.astype(np.float32), (0, 0), sig)
            + orig
            - cv2.GaussianBlur(orig, (0, 0), sig),
            0,
            255,
        ).astype(np.uint8)
    # oval mask (landmarks), eroded and feathered, in crop coordinates
    p = pts - (x0, y0)
    oval = lmk.polygon_mask(p, lmk.FACE_OVAL, (ch, cw))
    er = max(1, int(round(0.03 * fw)))
    oval = cv2.erode(oval, cv2.getStructuringElement(cv2.MORPH_ELLIPSE, (2 * er + 1, 2 * er + 1)))
    alpha = cv2.GaussianBlur(oval.astype(np.float32), (0, 0), max(1.5, 0.05 * fw))
    out = blend_u8(crop, back, strength)
    ys, xs = np.nonzero(alpha > 0.01)
    if len(ys) == 0:
        return None
    py0, py1 = max(0, ys.min() - 1), min(ch, ys.max() + 2)
    px0, px1 = max(0, xs.min() - 1), min(cw, xs.max() + 2)
    a8 = np.clip(np.rint(alpha[py0:py1, px0:px1] * 255), 0, 255).astype(np.uint8)
    rgba = np.dstack([out[py0:py1, px0:px1], a8])
    return RestoredFace(rgba, (x0 + px0, y0 + py0, px1 - px0, py1 - py0))
