"""Colour matching and multi-band blending (doc 03 section 5, steps 7-9)."""

from __future__ import annotations

import math

import cv2
import numpy as np


def _lab(rgb_u8: np.ndarray) -> np.ndarray:
    return cv2.cvtColor(rgb_u8.astype(np.float32) / 255.0, cv2.COLOR_RGB2LAB)


def _from_lab(lab: np.ndarray) -> np.ndarray:
    rgb = cv2.cvtColor(lab.astype(np.float32), cv2.COLOR_LAB2RGB)
    return np.clip(rgb * 255.0, 0, 255)


def boundary_ring(mask_bin: np.ndarray, radius: int) -> np.ndarray:
    """Band straddling the mask boundary (both sides, width ~2 * radius)."""
    k = cv2.getStructuringElement(cv2.MORPH_ELLIPSE, (2 * radius + 1, 2 * radius + 1))
    m = (mask_bin > 0).astype(np.uint8)
    return (cv2.dilate(m, k) - cv2.erode(m, k)) > 0


def color_match(
    base: np.ndarray,
    src: np.ndarray,
    ring: np.ndarray,
    sigma_field: float,
) -> tuple[np.ndarray, float]:
    """Match `src` (aligned to `base`) to `base` on the boundary `ring`, in LAB.

    1. L: mean / std matching (gain clamped to 0.85-1.18), a / b: mean shift;
    2. residual low-frequency correction: ring differences spread inwards by normalised
       convolution (sigma `sigma_field`), clamped to +-6 L / +-4 ab.
    Both stages use the central 80 % of the ring pixels (moving objects, hair strands, misaligned
    edges are outliers). Returns (matched uint8 RGB, mean |dE| on the ring before matching).
    """
    n = int(ring.sum())
    if n < 200:
        return src, 0.0
    lb, ls = _lab(base), _lab(src)
    diff0 = np.linalg.norm((lb - ls)[ring], axis=1)
    keep = diff0 <= np.percentile(diff0, 80)
    idx = np.nonzero(ring)
    sel = (idx[0][keep], idx[1][keep])
    out = ls.copy()
    for c in range(3):
        mb, ms = float(lb[..., c][sel].mean()), float(ls[..., c][sel].mean())
        if c == 0:
            sb, ss = float(lb[..., 0][sel].std()), float(ls[..., 0][sel].std())
            g = float(np.clip(sb / ss, 0.85, 1.18)) if ss > 1e-3 else 1.0
            out[..., 0] = (ls[..., 0] - ms) * g + mb
        else:
            out[..., c] = ls[..., c] + (mb - ms)
    # residual field from the ring, computed at <= 1/4 scale (very smooth by construction)
    k = 0.25 if max(ring.shape) > 400 else 1.0
    sh, sw = max(1, round(ring.shape[0] * k)), max(1, round(ring.shape[1] * k))
    w = np.zeros(ring.shape, np.float32)
    w[sel] = 1.0
    wk = cv2.resize(w, (sw, sh), interpolation=cv2.INTER_AREA) if k < 1 else w
    sig = max(2.0, sigma_field) * k
    den = cv2.GaussianBlur(wk, (0, 0), sig)
    lim = np.array([6.0, 4.0, 4.0], np.float32)
    gain = np.clip(den * 4.0, 0, 1)
    if k < 1:
        gain = cv2.resize(gain, (ring.shape[1], ring.shape[0]), interpolation=cv2.INTER_LINEAR)
    for c in range(3):
        r = np.where(w > 0, lb[..., c] - out[..., c], 0.0).astype(np.float32)
        rk = cv2.resize(r, (sw, sh), interpolation=cv2.INTER_AREA) if k < 1 else r
        field = cv2.GaussianBlur(rk, (0, 0), sig) / np.maximum(den, 1e-4)
        if k < 1:
            field = cv2.resize(
                field, (ring.shape[1], ring.shape[0]), interpolation=cv2.INTER_LINEAR
            )
        out[..., c] += np.clip(field, -lim[c], lim[c]) * gain
    return np.rint(_from_lab(out)).astype(np.uint8), float(diff0.mean())


def pyramid_levels(face_w: float, shape: tuple[int, int]) -> int:
    lv = round(math.log2(max(8.0, 0.12 * face_w)))
    cap = int(math.log2(max(8, min(shape)))) - 2
    return int(np.clip(min(lv, cap), 2, 6))


def multiband_blend(a: np.ndarray, b: np.ndarray, alpha: np.ndarray, levels: int) -> np.ndarray:
    """Laplacian-pyramid blend: result = alpha * b + (1 - alpha) * a, per frequency band.

    a, b uint8 HxWx3; alpha float [0, 1] HxW (Gaussian-pyramid-ed per level). Returns uint8.
    """
    h, w = alpha.shape
    # pad to a multiple of 2**levels so every pyramid level halves exactly
    m = 1 << levels
    ph, pw = (-h) % m, (-w) % m
    fa = np.pad(a, ((0, ph), (0, pw), (0, 0)), mode="edge").astype(np.float32)
    fb = np.pad(b, ((0, ph), (0, pw), (0, 0)), mode="edge").astype(np.float32)
    al = np.pad(alpha, ((0, ph), (0, pw)), mode="edge").astype(np.float32)

    def gauss(x: np.ndarray) -> list[np.ndarray]:
        g = [x]
        for _ in range(levels):
            g.append(cv2.pyrDown(g[-1]))
        return g

    def lap(g: list[np.ndarray]) -> list[np.ndarray]:
        out = []
        for i in range(levels):
            up = cv2.pyrUp(g[i + 1], dstsize=(g[i].shape[1], g[i].shape[0]))
            out.append(g[i] - up)
        out.append(g[-1])
        return out

    ga, gb, gm = gauss(fa), gauss(fb), gauss(al)
    la, lb = lap(ga), lap(gb)
    blended = [
        (lb[i] * (gm[i][..., None] if lb[i].ndim == 3 else gm[i]))
        + (la[i] * (1 - (gm[i][..., None] if la[i].ndim == 3 else gm[i])))
        for i in range(levels + 1)
    ]
    cur = blended[-1]
    for i in range(levels - 1, -1, -1):
        cur = cv2.pyrUp(cur, dstsize=(blended[i].shape[1], blended[i].shape[0])) + blended[i]
    return np.clip(np.rint(cur[:h, :w]), 0, 255).astype(np.uint8)


def _gray(rgb: np.ndarray) -> np.ndarray:
    return cv2.cvtColor(rgb, cv2.COLOR_RGB2GRAY).astype(np.float32) / 255.0


def _grad(g: np.ndarray) -> np.ndarray:
    gx = cv2.Sobel(g, cv2.CV_32F, 1, 0, ksize=3)
    gy = cv2.Sobel(g, cv2.CV_32F, 0, 1, ksize=3)
    return np.hypot(gx, gy)


def seam_metrics(
    base: np.ndarray, src: np.ndarray, comp: np.ndarray, ring: np.ndarray
) -> tuple[float, float]:
    """(gradient discontinuity, ring misfit) of a blend, both on the boundary ring.

    ring misfit   mean |luma(base) - luma(src matched)| on the ring: how well the two frames agree
                  where they are stitched (alignment + colour), 0-1.
    discontinuity mean gradient magnitude of the composite on the ring relative to the larger of
                  the two inputs' (a visible seam adds gradient that neither input has): ratio - 1,
                  clipped at 0.
    """
    if int(ring.sum()) < 50:
        return 0.0, 0.0
    gb, gs, gc = _gray(base), _gray(src), _gray(comp)
    misfit = float(np.abs(gb - gs)[ring].mean())
    ref = np.maximum(
        cv2.GaussianBlur(_grad(gb), (0, 0), 1.0), cv2.GaussianBlur(_grad(gs), (0, 0), 1.0)
    )
    gcc = cv2.GaussianBlur(_grad(gc), (0, 0), 1.0)
    ratio = float(gcc[ring].mean() / max(ref[ring].mean(), 1e-4))
    return max(0.0, ratio - 1.0), misfit
