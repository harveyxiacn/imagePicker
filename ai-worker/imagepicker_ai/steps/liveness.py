"""Cheap "is this face a real person?" heuristic (docs/03 §3.3, issue tag `bystander`).

Faces on statues, posters, screens and printed matter must not become bystanders (nobody can be
asked to step aside, and "remove bystander" must not inpaint a monument). The check looks only at
the pixels inside the YuNet box, no extra model:

  sat    mean HSV saturation of the inner face             grey stone / bronze / b&w print  -> low
  skin   share of pixels inside the YCrCb skin box          statues, cold prints             -> low
  tex    robust std (MAD) of the face minus its blur        skin has pores / sensor noise;
         (sigma 1.5 px on a 64 px crop)                     prints and renders are smooth    -> low
  plane  R^2 of a plane fitted to the heavily blurred       3D shading (cheeks, nose) leaves a
         luminance (sigma 6 px)                             residual; a flat print is planar -> high

  artificial = W_SAT*a_sat + W_SKIN*a_skin + W_TEX*a_tex + W_PLANE*a_plane   (each a_* is a 0-1
  smoothstep, 1 = looks artificial); `likely_artificial` = artificial >= ARTIFICIAL_AT.

A face that is flat but has skin colour and saturation scores at most W_TEX + W_PLANE = 0.5, below
the threshold: a smooth, well lit real face (retouched, low ISO) is never flagged by texture alone,
and the price is that a smooth, colourful poster is missed. In a monochrome photo (mean image
saturation below MONO_SAT) the colour cues carry no information, so only texture and planarity
count and the score is capped below the threshold: nothing is flagged there. Faces smaller than
MIN_SIDE px have too little evidence and return None (treated as real).

All constants are first guesses; `bench/eval-bystander.py` measures them on a labelled library.
"""

from __future__ import annotations

import cv2
import numpy as np

CROP = 64
MIN_SIDE = 24  # px of the face box in the analysed image
INNER = 0.70  # share of the box kept (skips hair / background corners)
W_SAT, W_SKIN, W_TEX, W_PLANE = 0.25, 0.25, 0.30, 0.20
ARTIFICIAL_AT = 0.55
MONO_SAT = 0.07  # image mean saturation below this: colour cues are ignored
MONO_CAP = 0.5
SAT_LO, SAT_HI = 0.08, 0.22  # face saturation: <= LO artificial, >= HI natural
SKIN_LO, SKIN_HI = 0.15, 0.55  # share of skin-tone pixels
TEX_LO, TEX_HI = 0.6, 2.5  # robust std of the high-pass residual, 0-255 grey levels
PLANE_LO, PLANE_HI = 0.85, 0.98  # R^2 of the plane fit
# YCrCb skin box (OpenCV 8-bit: Cr, Cb), the classic Chai-Ngan range
CR_RANGE = (133, 173)
CB_RANGE = (77, 127)


def _smooth(x: float, lo: float, hi: float) -> float:
    t = min(1.0, max(0.0, (x - lo) / (hi - lo)))
    return t * t * (3 - 2 * t)


def image_saturation(rgb: np.ndarray) -> float:
    """Mean HSV saturation (0-1) of a subsampled image."""
    sub = np.ascontiguousarray(rgb[::4, ::4])
    return float(cv2.cvtColor(sub, cv2.COLOR_RGB2HSV)[..., 1].mean() / 255.0)


def _crop(rgb: np.ndarray, bbox: tuple[float, float, float, float]) -> np.ndarray | None:
    h, w = rgb.shape[:2]
    x, y, bw, bh = bbox
    if min(bw * w, bh * h) < MIN_SIDE:
        return None
    cx, cy = (x + bw / 2) * w, (y + bh / 2) * h
    hw, hh = bw * w * INNER / 2, bh * h * INNER / 2
    x0, x1 = max(0, int(cx - hw)), min(w, int(cx + hw))
    y0, y1 = max(0, int(cy - hh)), min(h, int(cy + hh))
    if x1 - x0 < 8 or y1 - y0 < 8:
        return None
    crop = rgb[y0:y1, x0:x1]
    interp = cv2.INTER_AREA if max(crop.shape[:2]) > CROP else cv2.INTER_LINEAR
    return cv2.resize(crop, (CROP, CROP), interpolation=interp)


def plane_r2(gray: np.ndarray) -> float:
    """R^2 of the least-squares plane through a grey image (1.0 for a flat or planar one)."""
    h, w = gray.shape
    yy, xx = np.mgrid[0:h, 0:w]
    a = np.stack([xx.ravel(), yy.ravel(), np.ones(h * w)], axis=1).astype(np.float64)
    z = gray.ravel().astype(np.float64)
    coef, *_ = np.linalg.lstsq(a, z, rcond=None)
    res = z - a @ coef
    tot = float(((z - z.mean()) ** 2).sum())
    if tot < 1e-6:
        return 1.0
    return float(1.0 - (res**2).sum() / tot)


def face_features(crop: np.ndarray) -> dict[str, float]:
    hsv = cv2.cvtColor(crop, cv2.COLOR_RGB2HSV)
    ycc = cv2.cvtColor(crop, cv2.COLOR_RGB2YCrCb)
    cr, cb = ycc[..., 1], ycc[..., 2]
    skin = (cr >= CR_RANGE[0]) & (cr <= CR_RANGE[1]) & (cb >= CB_RANGE[0]) & (cb <= CB_RANGE[1])
    gray = cv2.cvtColor(crop, cv2.COLOR_RGB2GRAY).astype(np.float32)
    resid = gray - cv2.GaussianBlur(gray, (0, 0), 1.5)
    tex = 1.4826 * float(np.median(np.abs(resid - np.median(resid))))
    return {
        "sat": float(hsv[..., 1].mean() / 255.0),
        "skin": float(skin.mean()),
        "tex": tex,
        "plane": plane_r2(cv2.GaussianBlur(gray, (0, 0), 6.0)),
    }


def artificial_confidence(feat: dict[str, float], image_sat: float | None = None) -> float:
    """0-1 confidence that the face is not a living person, from `face_features`."""
    a_tex = 1.0 - _smooth(feat["tex"], TEX_LO, TEX_HI)
    a_plane = _smooth(feat["plane"], PLANE_LO, PLANE_HI)
    if image_sat is not None and image_sat < MONO_SAT:
        return min(MONO_CAP, (W_TEX * a_tex + W_PLANE * a_plane) / (W_TEX + W_PLANE) * MONO_CAP)
    a_sat = 1.0 - _smooth(feat["sat"], SAT_LO, SAT_HI)
    a_skin = 1.0 - _smooth(feat["skin"], SKIN_LO, SKIN_HI)
    return W_SAT * a_sat + W_SKIN * a_skin + W_TEX * a_tex + W_PLANE * a_plane


def face_artificial(
    rgb: np.ndarray,
    bbox: tuple[float, float, float, float],
    image_sat: float | None = None,
) -> float | None:
    """Artificial-face confidence 0-1 for a normalised [x, y, w, h] box, None when too small."""
    crop = _crop(rgb, bbox)
    if crop is None:
        return None
    return artificial_confidence(face_features(crop), image_sat)
