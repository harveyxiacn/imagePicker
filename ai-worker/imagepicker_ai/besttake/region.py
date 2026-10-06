"""Blend region of a Best Take patch: face + hair + neck (doc 03 section 5, step 6).

Everything is computed on the base crop (base pixel grid), for both frames:

  oval     landmark face oval (forehead to chin) - always part of the region;
  parts    selfie-multiclass face-skin + hair + (body-skin below the chin) of the base crop and of
           the aligned source crop, restricted to the target person's matte (BiRefNet
           `select_person`) when available - the union of both frames removes ghosts of whichever
           hair / neck outline differs between them, and the seam falls into the background;
  zone     elliptical limit around the oval so long hair / shoulders do not drag the region away;
  others   other people's faces (+ margin) are cut out, except inside the target's own oval.
Without segmentation models the region degrades to the oval grown by 12 % + a neck trapezoid.
"""

from __future__ import annotations

from dataclasses import dataclass

import cv2
import numpy as np

from ..beauty import landmarks as lmk
from ..masks.nets import CLS_BODY_SKIN, CLS_FACE_SKIN, CLS_HAIR


@dataclass
class Region:
    binary: np.ndarray  # uint8 {0, 1}: where the source replaces the base
    oval: np.ndarray  # uint8 {0, 1}: union of both landmark ovals
    face_w: float
    face_h: float


def _ell(r: int) -> np.ndarray:
    r = max(1, int(r))
    return cv2.getStructuringElement(cv2.MORPH_ELLIPSE, (2 * r + 1, 2 * r + 1))


def oval_of(pts: np.ndarray, shape: tuple[int, int]) -> np.ndarray:
    return lmk.polygon_mask(pts, lmk.FACE_OVAL, shape)


def neck_zone(pts: np.ndarray, shape: tuple[int, int], fw: float, fh: float) -> np.ndarray:
    """Trapezoid below the chin: where body-skin counts as neck."""
    chin = pts[152]
    jl, jr = pts[172], pts[397]
    cx = chin[0]
    half_top = max(0.5 * fw * 0.8, abs(jr[0] - jl[0]) / 2 * 0.9)
    poly = np.array(
        [
            [cx - half_top, chin[1] - 0.25 * fh],
            [cx + half_top, chin[1] - 0.25 * fh],
            [cx + half_top * 1.05, chin[1] + 0.42 * fh],
            [cx - half_top * 1.05, chin[1] + 0.42 * fh],
        ]
    )
    m = np.zeros(shape, np.uint8)
    cv2.fillPoly(m, [np.round(poly * 16).astype(np.int32)], 1, lineType=cv2.LINE_AA, shift=4)
    return m


def zone_mask(oval: np.ndarray, fw: float, fh: float) -> np.ndarray:
    """Half-ellipses (up / down) around the oval: room for hair above, neck and chin below."""
    ys, xs = np.nonzero(oval)
    h, w = oval.shape
    cx, cy = float(xs.mean()), float(ys.mean())
    ow = float(xs.max() - xs.min() + 1)
    oh = float(ys.max() - ys.min() + 1)
    yy, xx = np.mgrid[0:h, 0:w].astype(np.float32)
    dx = (xx - cx) / (0.85 * ow)
    up = (yy - cy) / (0.95 * oh)
    dn = (yy - cy) / (0.85 * oh)
    dy = np.where(yy < cy, up, dn)
    return (dx * dx + dy * dy <= 1.0).astype(np.uint8)


def _frame_region(
    probs: np.ndarray | None, matte: np.ndarray | None, neck: np.ndarray
) -> np.ndarray | None:
    if probs is None:
        return None
    face = probs[..., CLS_FACE_SKIN] + probs[..., CLS_HAIR]
    body = probs[..., CLS_BODY_SKIN] * neck
    r = np.clip(face + body, 0, 1)
    if matte is not None:
        r = r * np.clip(matte * 1.5, 0, 1)
    return (r > 0.5).astype(np.uint8)


def _keep_component(m: np.ndarray, seed_mask: np.ndarray) -> np.ndarray:
    n, lab = cv2.connectedComponents(m, connectivity=8)
    if n <= 2:
        return m
    cnt = np.bincount(lab[seed_mask > 0].ravel(), minlength=n)
    cnt[0] = 0
    keep = cnt >= max(1, 0.1 * cnt.max())
    keep[0] = False
    return keep[lab].astype(np.uint8)


def fill_holes(m: np.ndarray) -> np.ndarray:
    ff = m.copy()
    h, w = m.shape
    pad = cv2.copyMakeBorder(ff, 1, 1, 1, 1, cv2.BORDER_CONSTANT, value=0)
    mask = np.zeros((h + 4, w + 4), np.uint8)
    cv2.floodFill(pad, mask, (0, 0), 1)
    holes = pad[1:-1, 1:-1] == 0
    out = m.copy()
    out[holes] = 1
    return out


def build_region(
    base_pts: np.ndarray,
    src_pts_in_base: np.ndarray,
    shape: tuple[int, int],
    probs_base: np.ndarray | None = None,
    probs_src: np.ndarray | None = None,
    matte_base: np.ndarray | None = None,
    matte_src: np.ndarray | None = None,
    others: np.ndarray | None = None,
) -> Region:
    """All landmark inputs in base-crop pixels; `probs_*` HxWx6, `matte_*` HxW in [0, 1]."""
    oval_b = oval_of(base_pts, shape)
    oval_s = oval_of(src_pts_in_base, shape)
    oval = oval_b | oval_s
    ys, xs = np.nonzero(oval_b)
    fw = float(xs.max() - xs.min() + 1)
    fh = float(ys.max() - ys.min() + 1)

    zone = zone_mask(oval_b, fw, fh)
    neck_b = neck_zone(base_pts, shape, fw, fh)
    neck_s = neck_zone(src_pts_in_base, shape, fw, fh)
    rb = _frame_region(probs_base, matte_base, neck_b)
    rs = _frame_region(probs_src, matte_src, neck_s)
    core = cv2.dilate(oval, _ell(0.02 * fw))
    if rb is None and rs is None:
        # landmark-only fallback: oval grown by 12 % + neck trapezoid
        region = cv2.dilate(oval, _ell(0.12 * fw)) | neck_b | neck_s
    else:
        region = core.copy()
        for r in (rb, rs):
            if r is not None:
                region |= r & zone
    region &= cv2.dilate(zone, _ell(0.02 * fw))
    region = cv2.morphologyEx(region, cv2.MORPH_CLOSE, _ell(0.04 * fw))
    region = fill_holes(region)
    region = _keep_component(region, oval_b)
    region = cv2.dilate(region, _ell(0.03 * fw))
    if others is not None:
        cut = (others > 0.3).astype(np.uint8)
        cut = cv2.dilate(cut, _ell(0.04 * fw))
        cut[oval > 0] = 0  # a face overlapping the target's own oval is an occlusion, not a cut
        region[cut > 0] = 0
        region = _keep_component(region, oval_b)
    return Region(region.astype(np.uint8), oval.astype(np.uint8), fw, fh)
