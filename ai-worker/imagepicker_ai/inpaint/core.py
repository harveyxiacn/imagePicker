"""Inpainting around a hole mask with a fixed-size fill network (LaMa 512 / SDXL 1024).

For every cluster of the mask:
  * a square crop around the mask bbox with context (side = bbox + 2 * max(96 px, 40 %), at least
    the network size, so small holes are filled at native resolution without any resampling),
  * resized to the network size, filled, resized back,
  * the fill is colour-corrected with the offset measured on a ring around the hole (normalised
    convolution), and film grain of the surroundings is added back (the networks produce smooth
    texture),
  * pasted into the crop with a few-pixel feather. Outside the hole the patch equals the photo.
The patch (RGB = photo with the holes filled, alpha = feathered mask) covers the mask bbox + margin.
"""

from __future__ import annotations

import math
from collections.abc import Callable
from dataclasses import dataclass, field
from typing import Any

import cv2
import numpy as np

FillFn = Callable[[np.ndarray, np.ndarray], np.ndarray]  # (rgb u8 s x s, mask f32 s x s) -> rgb u8


@dataclass
class FillEngine:
    size: int  # network input edge (512 for LaMa)
    fn: FillFn
    min_context: int = 96  # px of context on each side of the hole


@dataclass
class InpaintOut:
    patch: np.ndarray  # HxWx4 uint8 RGBA
    rect: tuple[int, int, int, int]  # x, y, w, h in photo pixels
    info: dict[str, Any] = field(default_factory=dict)


def _ell(r: int) -> np.ndarray:
    r = max(1, int(r))
    return cv2.getStructuringElement(cv2.MORPH_ELLIPSE, (2 * r + 1, 2 * r + 1))


def clusters(mask_bin: np.ndarray, gap: int) -> list[tuple[int, int, int, int]]:
    """Bounding boxes (x0, y0, x1, y1) of the mask's connected groups; holes closer than `gap` px merge."""
    h, w = mask_bin.shape
    s = max(1, int(round(max(h, w) / 1024)))
    small = cv2.resize(mask_bin, (max(1, w // s), max(1, h // s)), interpolation=cv2.INTER_AREA) > 0
    small = cv2.dilate(small.astype(np.uint8), _ell(max(1, gap // s)))
    n, lab = cv2.connectedComponents(small, connectivity=8)
    boxes = []
    for i in range(1, n):
        ys, xs = np.nonzero(lab == i)
        x0, x1 = max(0, xs.min() * s), min(w, (xs.max() + 1) * s)
        y0, y1 = max(0, ys.min() * s), min(h, (ys.max() + 1) * s)
        sub = mask_bin[y0:y1, x0:x1]
        yy, xx = np.nonzero(sub)
        if len(yy) == 0:
            continue
        boxes.append(
            (x0 + int(xx.min()), y0 + int(yy.min()), x0 + int(xx.max()) + 1, y0 + int(yy.max()) + 1)
        )
    return boxes


def _square(crop: np.ndarray, side: int, border: int) -> np.ndarray:
    ph, pw = side - crop.shape[0], side - crop.shape[1]
    if ph <= 0 and pw <= 0:
        return crop
    mode = border if min(crop.shape[:2]) > max(ph, pw) else cv2.BORDER_REPLICATE
    return cv2.copyMakeBorder(crop, 0, max(ph, 0), 0, max(pw, 0), mode)


def _noise_sigma(luma: np.ndarray, region: np.ndarray) -> float:
    """Robust noise sigma: MAD of the luma high-pass (texture / edges inflate std, not the MAD)."""
    hp = luma - cv2.GaussianBlur(luma, (0, 0), 1.0)
    v = hp[region]
    if v.size < 50:
        return 0.0
    return float(np.median(np.abs(v - np.median(v)))) * 1.4826


def fill_cluster(
    img: np.ndarray,
    mask: np.ndarray,
    box: tuple[int, int, int, int],
    engine: FillEngine,
    grain: bool = True,
    rng: np.random.Generator | None = None,
) -> tuple[tuple[int, int, int, int], np.ndarray]:
    """Fill one cluster. Returns (crop rect x0, y0, x1, y1, crop RGB with the hole filled)."""
    h, w = img.shape[:2]
    bx0, by0, bx1, by1 = box
    bw, bh = bx1 - bx0, by1 - by0
    side = max(bw, bh)
    ctx = max(engine.min_context, int(0.4 * side))
    S = int(min(max(engine.size, side + 2 * ctx), 4096))
    cw, ch = min(S, w), min(S, h)
    cx, cy = (bx0 + bx1) // 2, (by0 + by1) // 2
    x0 = int(np.clip(cx - cw // 2, 0, w - cw))
    y0 = int(np.clip(cy - ch // 2, 0, h - ch))
    crop = np.ascontiguousarray(img[y0 : y0 + ch, x0 : x0 + cw])
    mcrop = np.ascontiguousarray(mask[y0 : y0 + ch, x0 : x0 + cw])
    Ssq = max(cw, ch)
    crop_sq = _square(crop, Ssq, cv2.BORDER_REFLECT_101)
    m_sq = _square(mcrop, Ssq, cv2.BORDER_CONSTANT)
    t = engine.size
    shrink = Ssq > t
    crop_t = cv2.resize(
        crop_sq, (t, t), interpolation=cv2.INTER_AREA if shrink else cv2.INTER_CUBIC
    )
    m_t = cv2.resize(
        m_sq.astype(np.float32),
        (t, t),
        interpolation=cv2.INTER_AREA if shrink else cv2.INTER_LINEAR,
    )
    m_t = (m_t > 0.15).astype(np.float32)
    out_t = engine.fn(crop_t, m_t)
    out = cv2.resize(out_t, (Ssq, Ssq), interpolation=cv2.INTER_CUBIC if shrink else cv2.INTER_AREA)
    out = out[:ch, :cw]

    hole = mcrop > 0
    # colour correction from a ring just outside the hole
    ring_r = max(4, int(0.03 * Ssq))
    hole_u8 = hole.astype(np.uint8)
    ring = (cv2.dilate(hole_u8, _ell(ring_r)) > 0) & ~(cv2.dilate(hole_u8, _ell(2)) > 0)
    # other holes' ring pixels are fine: they show the photo around them
    outf = out.astype(np.float32)
    origf = crop.astype(np.float32)
    if ring.sum() > 100:
        wring = ring.astype(np.float32)
        sig = max(3.0, 0.08 * Ssq)
        den = cv2.GaussianBlur(wring, (0, 0), sig)
        corr = np.zeros_like(outf)
        for c in range(3):
            r = np.where(ring, origf[..., c] - outf[..., c], 0.0).astype(np.float32)
            corr[..., c] = np.clip(
                cv2.GaussianBlur(r, (0, 0), sig) / np.maximum(den, 1e-4), -24, 24
            )
        outf = outf + corr * np.clip(den * 6.0, 0, 1)[..., None]

    # film grain of the surroundings
    if grain and ring.sum() > 100:
        luma_o = cv2.cvtColor(crop, cv2.COLOR_RGB2GRAY).astype(np.float32)
        sn = _noise_sigma(luma_o, ring)
        luma_f = cv2.cvtColor(np.clip(outf, 0, 255).astype(np.uint8), cv2.COLOR_RGB2GRAY).astype(
            np.float32
        )
        so = _noise_sigma(luma_f, hole)
        add = min(math.sqrt(max(sn * sn - so * so, 0.0)), 6.0) * 0.8
        if add > 0.3:
            rng = rng or np.random.default_rng(0)
            n = cv2.GaussianBlur(rng.normal(0, 1, (ch, cw)).astype(np.float32), (0, 0), 0.5)
            n *= add / max(float(n.std()), 1e-3)
            chroma = rng.normal(0, 1, (ch, cw, 3)).astype(np.float32) * (0.25 * add)
            outf = outf + n[..., None] + chroma

    sig_in = max(1.0, 0.0015 * Ssq)
    mh = cv2.GaussianBlur(
        cv2.dilate(hole_u8, _ell(round(2 * sig_in))).astype(np.float32), (0, 0), sig_in
    )
    filled = origf * (1 - mh[..., None]) + np.clip(outf, 0, 255) * mh[..., None]
    return (x0, y0, x0 + cw, y0 + ch), np.clip(np.rint(filled), 0, 255).astype(np.uint8)


def inpaint_arrays(
    img: np.ndarray,
    mask: np.ndarray,
    engine: FillEngine,
    grain: bool = True,
) -> InpaintOut | None:
    """`img` HxWx3 uint8, `mask` HxW float [0, 1] (> 0.5 = remove). None when the mask is empty."""
    h, w = img.shape[:2]
    long_edge = max(h, w)
    mbin = (mask > 0.5).astype(np.uint8)
    if not mbin.any():
        return None
    # eat the object's anti-aliased halo
    mbin = cv2.dilate(mbin, _ell(int(np.clip(round(0.002 * long_edge), 2, 8))))
    mfill = mbin.astype(np.float32)
    boxes = clusters(mbin, gap=64)
    ys, xs = np.nonzero(mbin)
    sigma = max(1.5, 0.0025 * long_edge)
    margin = int(math.ceil(4 * sigma)) + 4
    wx0, wx1 = max(0, int(xs.min()) - margin), min(w, int(xs.max()) + 1 + margin)
    wy0, wy1 = max(0, int(ys.min()) - margin), min(h, int(ys.max()) + 1 + margin)
    patch = np.ascontiguousarray(img[wy0:wy1, wx0:wx1]).copy()
    rng = np.random.default_rng(1234)
    for box in boxes:
        (x0, y0, x1, y1), filled = fill_cluster(img, mfill, box, engine, grain, rng)
        ix0, iy0, ix1, iy1 = max(x0, wx0), max(y0, wy0), min(x1, wx1), min(y1, wy1)
        if ix1 > ix0 and iy1 > iy0:
            patch[iy0 - wy0 : iy1 - wy0, ix0 - wx0 : ix1 - wx0] = filled[
                iy0 - y0 : iy1 - y0, ix0 - x0 : ix1 - x0
            ]
    win = mbin[wy0:wy1, wx0:wx1]
    alpha = cv2.GaussianBlur(
        cv2.dilate(win, _ell(round(2 * sigma))).astype(np.float32), (0, 0), sigma
    )
    alpha = np.clip(alpha * 1.02, 0, 1)
    ay, ax = np.nonzero(alpha > 0.004)
    py0, py1 = int(ay.min()), int(ay.max()) + 1
    px0, px1 = int(ax.min()), int(ax.max()) + 1
    a8 = np.clip(np.rint(alpha[py0:py1, px0:px1] * 255), 0, 255).astype(np.uint8)
    rgba = np.dstack([patch[py0:py1, px0:px1], a8])
    return InpaintOut(
        rgba,
        (wx0 + px0, wy0 + py0, px1 - px0, py1 - py0),
        {"clusters": len(boxes), "engine": engine.size},
    )
