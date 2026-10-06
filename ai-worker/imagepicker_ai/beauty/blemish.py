"""Blemish detection: small local dark / red spots on a person's facial skin.

Method (all on the face crop, CIE Lab, L in 0..100):
  1. masked Gaussian pyramid of L and a* (normalised convolution with the skin mask, so eyes,
     brows, lips, hair and the background never leak into the blur),
  2. difference of Gaussians at SCALES blob radii between 1.2 % and 5 % of the face width
     (sigma = r / sqrt 2, surround = 1.6 sigma): spots that are darker (L) or redder (a*) than
     their surroundings give a positive response,
  3. response / robust noise level (1.4826 * MAD of the response over the skin of this face), so
     the threshold adapts to skin texture; score = hypot(zL+, za+),
  4. 3x3x3 non-maximum suppression over (scale, y, x), greedy removal of overlapping blobs,
  5. rejection: absolute contrast below ~2.5 Lab units, less than 85 % skin / any exclusion zone
     (eyes, brows, lips, nostrils) / hair inside the disk, outside the face oval, and edge / line
     structures (Hessian of the winning channel at sigma: both eigenvalues must have the blob
     sign and lambda2 / lambda1 >= 0.3, which drops lashes, hair strands, nasolabial folds),
  6. cap at `max_count` strongest spots.
Returns pixel `(x, y, r)`; the caller normalises (r relative to the image long edge).
"""

from __future__ import annotations

import cv2
import numpy as np

R_MIN_FRAC = 0.012  # blob radius range, in face widths
R_MAX_FRAC = 0.050
N_SCALES = 4
SURROUND = 1.6
Z_MIN = 4.5
MIN_CONTRAST = 2.5  # Lab units (L or a*)
MIN_EIG_RATIO = 0.3
MIN_SKIN_IN_DISK = 0.85
MIN_FACE_PX = 64
MAX_COUNT = 24
WORK_FACE_PX = 320  # faces wider than this are analysed at this width (spots scale with the face)
NOISE_FLOOR = 0.35  # Lab units; keeps perfectly smooth patches from triggering
R_OUT_SCALE = 1.25  # reported radius = blob radius * this (covers the halo for inpainting)


def _masked_blur(
    ch: np.ndarray, w: np.ndarray, sigma: float, den: dict[float, np.ndarray] | None = None
) -> np.ndarray:
    """Gaussian blur of `ch` normalised by the blurred weights `w` (`den` caches those per sigma)."""
    num = cv2.GaussianBlur(ch * w, (0, 0), sigma)
    if den is None:
        d = cv2.GaussianBlur(w, (0, 0), sigma)
    else:
        d = den.get(sigma)
        if d is None:
            d = den[sigma] = cv2.GaussianBlur(w, (0, 0), sigma)
    return np.where(d > 1e-3, num / np.maximum(d, 1e-3), 0.0).astype(np.float32)


def _hessian_at(img: np.ndarray, x: int, y: int) -> tuple[float, float]:
    """Eigenvalues (larger, smaller) of the 2x2 Hessian of `img` at a pixel (central differences)."""
    h, w = img.shape
    xm, xp = max(0, x - 1), min(w - 1, x + 1)
    ym, yp = max(0, y - 1), min(h - 1, y + 1)
    c = img[y, x]
    lxx = (img[y, xp] - 2 * c + img[y, xm]) / max(1, (xp - xm) / 2) ** 2
    lyy = (img[yp, x] - 2 * c + img[ym, x]) / max(1, (yp - ym) / 2) ** 2
    lxy = (img[yp, xp] - img[yp, xm] - img[ym, xp] + img[ym, xm]) / (
        max(1, xp - xm) * max(1, yp - ym)
    )
    tr, det = lxx + lyy, lxx * lyy - lxy * lxy
    disc = max(0.0, tr * tr / 4 - det) ** 0.5
    return tr / 2 + disc, tr / 2 - disc


def _disk(
    shape: tuple[int, int], cx: float, cy: float, r: float
) -> tuple[slice, slice, np.ndarray]:
    h, w = shape
    x0, x1 = max(0, int(cx - r - 1)), min(w, int(cx + r + 2))
    y0, y1 = max(0, int(cy - r - 1)), min(h, int(cy + r + 2))
    yy, xx = np.mgrid[y0:y1, x0:x1]
    m = (xx - cx) ** 2 + (yy - cy) ** 2 <= r * r
    return slice(y0, y1), slice(x0, x1), m


def detect_blemishes(
    rgb: np.ndarray,
    skin: np.ndarray,
    face_box: tuple[float, float, float, float],
    region: np.ndarray | None = None,
    exclude: np.ndarray | None = None,
    hair: np.ndarray | None = None,
    max_count: int = MAX_COUNT,
) -> list[tuple[float, float, float]]:
    """Blemishes of one face as pixel `(x, y, r)` in the coordinates of `rgb`.

    skin     float HxW in [0, 1], this person's skin (eyes / brows / lips already removed)
    face_box (x, y, w, h) pixels
    region   optional HxW {0, 1} area where blemishes may be reported (the face oval)
    exclude  optional HxW {0, 1} zones that must stay untouched (eyes, brows, lips, nostrils)
    hair     optional float HxW hair probability
    """
    ih, iw = rgb.shape[:2]
    fx, fy, fw, fh = face_box
    if fw < MIN_FACE_PX:
        return []
    pad = 0.2
    x0, y0 = max(0, int(fx - pad * fw)), max(0, int(fy - pad * fh))
    x1, y1 = min(iw, int(np.ceil(fx + (1 + pad) * fw))), min(ih, int(np.ceil(fy + (1 + pad) * fh)))
    if x1 - x0 < 8 or y1 - y0 < 8:
        return []

    sc = min(1.0, WORK_FACE_PX / fw)
    cw, ch = x1 - x0, y1 - y0
    tw, th = max(8, round(cw * sc)), max(8, round(ch * sc))
    if sc < 1.0:
        fw = fw * sc

    def crop(a: np.ndarray | None) -> np.ndarray | None:
        if a is None:
            return None
        c = a[y0:y1, x0:x1]
        if sc < 1.0:
            c = cv2.resize(c.astype(np.float32), (tw, th), interpolation=cv2.INTER_AREA)
        return c

    rgb_c = np.ascontiguousarray(rgb[y0:y1, x0:x1])
    if sc < 1.0:
        rgb_c = cv2.resize(rgb_c, (tw, th), interpolation=cv2.INTER_AREA)
    lab = cv2.cvtColor(rgb_c.astype(np.float32) / 255.0, cv2.COLOR_RGB2LAB)
    chan_l, chan_a = lab[..., 0], lab[..., 1]
    sk = np.clip(crop(skin), 0.0, 1.0).astype(np.float32)  # type: ignore[arg-type]
    reg = crop(region)
    exc = crop(exclude)
    hr = crop(hair)
    # blurs only see (confident) skin that is not an exclusion zone
    w = (sk > 0.5).astype(np.float32)
    if exc is not None:
        w = w * (1.0 - (exc > 0.01).astype(np.float32))
    stat_mask = cv2.erode(w, np.ones((3, 3), np.uint8)) > 0.5
    if reg is not None:
        stat_mask &= reg > 0.5
    if stat_mask.sum() < 200:
        return []

    r_lo, r_hi = R_MIN_FRAC * fw, R_MAX_FRAC * fw
    radii = np.geomspace(r_lo, r_hi, N_SCALES)
    z_stack, dl_stack, da_stack = [], [], []
    dens: dict[float, np.ndarray] = {}
    for r in radii:
        sg = max(0.7, r / np.sqrt(2))
        zs, dls = [], []
        for ch, sign in ((chan_l, -1.0), (chan_a, 1.0)):
            d = sign * (_masked_blur(ch, w, sg, dens) - _masked_blur(ch, w, SURROUND * sg, dens))
            vals = d[stat_mask]
            med = float(np.median(vals))
            noise = max(1.4826 * float(np.median(np.abs(vals - med))), NOISE_FLOOR)
            zs.append(np.maximum((d - med) / noise, 0.0))
            dls.append(d)
        z_stack.append(np.hypot(zs[0], zs[1]))
        dl_stack.append(dls[0])
        da_stack.append(dls[1])
    z = np.stack(z_stack)  # (S, H, W)

    # 3x3x3 non-maximum suppression
    k = np.ones((3, 3), np.uint8)
    dil = np.stack([cv2.dilate(p, k) for p in z])
    nbr = dil.copy()
    nbr[1:] = np.maximum(nbr[1:], dil[:-1])
    nbr[:-1] = np.maximum(nbr[:-1], dil[1:])
    peaks = (z >= nbr) & (z >= Z_MIN)
    if reg is not None:
        peaks &= (reg > 0.5)[None]
    peaks &= (w > 0)[None]
    si, ys, xs = np.nonzero(peaks)
    order = np.argsort(-z[si, ys, xs])

    # (the blob of a "uniform disk" of radius r peaks at sigma = r / sqrt 2: radii[] are blob radii)
    out: list[tuple[float, float, float, float]] = []
    for o in order:
        s, y, x = int(si[o]), int(ys[o]), int(xs[o])
        r = float(radii[s])
        if r < 1.5:
            continue
        if any((x - px) ** 2 + (y - py) ** 2 < max(r, pr) ** 2 for px, py, pr, _ in out):
            continue
        dl, da = float(dl_stack[s][y, x]), float(da_stack[s][y, x])
        if max(dl, da) < MIN_CONTRAST:
            continue
        ysl, xsl, disk = _disk(sk.shape, x, y, r)
        if sk[ysl, xsl][disk].mean() < MIN_SKIN_IN_DISK:
            continue
        ring = _disk(sk.shape, x, y, r + 0.02 * fw)
        if exc is not None and (exc[ring[0], ring[1]][ring[2]] > 0.01).any():
            continue
        if hr is not None:
            hd = _disk(sk.shape, x, y, 2 * r)
            if hr[hd[0], hd[1]][hd[2]].mean() > 0.15:
                continue
        # edge / line rejection on the winning channel at the detection scale
        sg = max(0.7, r / np.sqrt(2))
        img = -_masked_blur(chan_l, w, sg, dens) if dl >= da else _masked_blur(chan_a, w, sg, dens)
        # a blob is a *maximum* of img: both Hessian eigenvalues negative
        e1, e2 = _hessian_at(img, x, y)  # e1 >= e2
        if not (e1 < 0 and e2 < 0):
            continue
        if abs(e1) / abs(e2) < MIN_EIG_RATIO:  # |e1| is the smaller curvature
            continue
        out.append((x / sc + x0, y / sc + y0, r / sc, float(z[s, y, x])))
        if len(out) >= max_count:
            break
    return [(x, y, r * R_OUT_SCALE) for x, y, r, _ in out]
