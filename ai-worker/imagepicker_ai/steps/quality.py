"""Classical technical-quality metrics (doc 03 §3.1, "classic fallback" column). All scores 0-1.

Conventions: `sharpness` and `exposure` are *goodness* scores (1 = best); `noise` is a *level*
(0 = clean, 1 = very noisy) - lower is better. All metrics work on the analysis-size image;
for resolution independence the grayscale is first resampled so that its long edge is
REF_LONG_EDGE (=1024) px.

Sharpness
  g      = gray (0..255), lightly smoothed (sigma 0.7) so sensor noise does not read as detail
  lap    = Var[Laplacian(g)]                          (variance of Laplacian)
  ten    = mean(Gx^2 + Gy^2), Sobel 3x3               (Tenengrad)
  s      = 0.5 * (1 - exp(-lap / K_lap)) + 0.5 * (1 - exp(-ten / K_ten))   (K_lap=120, K_ten=1500;
           faces: weights 0.7/0.3, K_lap=80, K_ten=6000)
  sharpness_center  = s on the central 60% x 60% region
  sharpness_face    = mean of s over detected faces (crop resized to 128x128, central 60%, face constants)
  sharpness         = sharpness_face if faces else sharpness_center
Exposure
  Y      = 0.2126 R + 0.7152 G + 0.0722 B  in 0..1
  mean   = mean(Y)
  hi     = P(Y >= 250/255)   (clipped highlights),  lo = P(Y <= 5/255)  (crushed shadows)
  exposure = clamp( 1 - |mean - 0.46| / 0.5 - 3*max(0, hi - 0.02) - 2*max(0, lo - 0.05), 0, 1 )
Noise
  Immerkaer's Laplacian-difference residual r = (g * [[1,-2,1],[-2,4,-2],[1,-2,1]]) has std 6*sigma
  for white noise sigma. Per 16x16 block: sigma_b = 1.4826 * MAD(r_block) / 6 (MAD is robust to edges).
  Only the 25% flattest blocks (lowest mean gradient, not clipped) are used: sigma = median(sigma_b).
  noise = clamp(sigma / 12, 0, 1)
"""

from __future__ import annotations

import cv2
import numpy as np

REF_LONG_EDGE = 1024
K_LAP = 120.0
K_TEN = 1500.0
K_LAP_FACE = 80.0
K_TEN_FACE = 6000.0
FACE_LAP_WEIGHT = 0.7  # inner-face Tenengrad has a high floor (feature edges), lean on Laplacian
FACE_CROP = 128
CENTER_MARGIN = 0.2
NOISE_FULL_SCALE_SIGMA = 12.0
_IMMERKAER = np.array([[1, -2, 1], [-2, 4, -2], [1, -2, 1]], dtype=np.float32)


def _clamp01(x: float) -> float:
    return float(min(1.0, max(0.0, x)))


def _norm_gray(rgb: np.ndarray) -> np.ndarray:
    gray = cv2.cvtColor(rgb, cv2.COLOR_RGB2GRAY)
    h, w = gray.shape
    long_edge = max(h, w)
    if abs(long_edge - REF_LONG_EDGE) > 0.1 * REF_LONG_EDGE:
        s = REF_LONG_EDGE / long_edge
        interp = cv2.INTER_AREA if s < 1 else cv2.INTER_LINEAR
        gray = cv2.resize(gray, (max(1, round(w * s)), max(1, round(h * s))), interpolation=interp)
    return gray


def focus_measures(gray: np.ndarray) -> tuple[float, float]:
    """(variance of Laplacian, Tenengrad = mean squared Sobel gradient) of a gray image."""
    g = gray.astype(np.float32)
    g = cv2.GaussianBlur(g, (0, 0), 0.7)
    lap = float(cv2.Laplacian(g, cv2.CV_32F, ksize=1).var())
    gx = cv2.Sobel(g, cv2.CV_32F, 1, 0, ksize=3)
    gy = cv2.Sobel(g, cv2.CV_32F, 0, 1, ksize=3)
    ten = float(np.mean(gx * gx + gy * gy))
    return lap, ten


def sharpness_score(lap: float, ten: float, face: bool = False) -> float:
    kl, kt, wl = (K_LAP_FACE, K_TEN_FACE, FACE_LAP_WEIGHT) if face else (K_LAP, K_TEN, 0.5)
    return _clamp01(wl * (1 - np.exp(-lap / kl)) + (1 - wl) * (1 - np.exp(-ten / kt)))


def center_region(gray: np.ndarray) -> np.ndarray:
    h, w = gray.shape
    my, mx = int(h * CENTER_MARGIN), int(w * CENTER_MARGIN)
    return gray[my : h - my, mx : w - mx]


def face_sharpness(rgb: np.ndarray, bbox_norm: tuple[float, float, float, float]) -> float | None:
    """Sharpness of a face given its normalized [x, y, w, h] box, measured on a 128x128 crop."""
    h, w = rgb.shape[:2]
    x, y, bw, bh = bbox_norm
    x0, y0 = max(0, int(x * w)), max(0, int(y * h))
    x1, y1 = min(w, int((x + bw) * w)), min(h, int((y + bh) * h))
    if x1 - x0 < 8 or y1 - y0 < 8:
        return None
    crop = cv2.cvtColor(rgb[y0:y1, x0:x1], cv2.COLOR_RGB2GRAY)
    interp = cv2.INTER_AREA if max(crop.shape) > FACE_CROP else cv2.INTER_LINEAR
    crop = cv2.resize(crop, (FACE_CROP, FACE_CROP), interpolation=interp)
    crop = center_region(crop)  # inner face (eyes/nose/mouth): skips hair/background contours
    lap, ten = focus_measures(crop)
    return sharpness_score(lap, ten, face=True)


def exposure_metrics(rgb: np.ndarray) -> dict[str, float]:
    # luma from a 1/4 subsample is plenty for histogram statistics
    sub = rgb[::2, ::2].astype(np.float32) / 255.0
    y = 0.2126 * sub[..., 0] + 0.7152 * sub[..., 1] + 0.0722 * sub[..., 2]
    mean = float(y.mean())
    hi = float((y >= 250 / 255).mean())
    lo = float((y <= 5 / 255).mean())
    score = _clamp01(1 - abs(mean - 0.46) / 0.5 - 3 * max(0.0, hi - 0.02) - 2 * max(0.0, lo - 0.05))
    return {
        "exposure": score,
        "mean_luminance": mean,
        "clipped_highlights": hi,
        "crushed_shadows": lo,
    }


def noise_sigma(gray: np.ndarray, block: int = 16, flat_fraction: float = 0.25) -> float:
    g = gray.astype(np.float32)
    h, w = g.shape
    h2, w2 = h // block * block, w // block * block
    if h2 < block * 2 or w2 < block * 2:
        return 0.0
    g = g[:h2, :w2]
    resp = cv2.filter2D(g, cv2.CV_32F, _IMMERKAER)
    sm = cv2.GaussianBlur(g, (0, 0), 1.0)
    gx = cv2.Sobel(sm, cv2.CV_32F, 1, 0, ksize=3)
    gy = cv2.Sobel(sm, cv2.CV_32F, 0, 1, ksize=3)
    grad = np.sqrt(gx * gx + gy * gy)

    def blocks(a: np.ndarray) -> np.ndarray:
        return (
            a.reshape(h2 // block, block, w2 // block, block)
            .transpose(0, 2, 1, 3)
            .reshape(-1, block * block)
        )

    rb, gb, mb = blocks(resp), blocks(grad), blocks(g)
    grad_mean = gb.mean(axis=1)
    bright = mb.mean(axis=1)
    valid = (bright > 8) & (bright < 247)  # clipped blocks have no noise information
    if valid.sum() < 4:
        valid[:] = True
    idx = np.flatnonzero(valid)
    n_pick = max(4, int(len(idx) * flat_fraction))
    pick = idx[np.argsort(grad_mean[idx])[:n_pick]]
    sel = rb[pick]
    med = np.median(sel, axis=1, keepdims=True)
    mad = np.median(np.abs(sel - med), axis=1)
    sigma_b = 1.4826 * mad / 6.0
    return float(np.median(sigma_b))


def analyze_quality(
    rgb: np.ndarray, face_boxes: list[tuple[float, float, float, float]] | None = None
) -> dict[str, float | None]:
    gray = _norm_gray(rgb)
    lap, ten = focus_measures(center_region(gray))
    s_center = sharpness_score(lap, ten)

    face_scores = [s for b in (face_boxes or []) if (s := face_sharpness(rgb, b)) is not None]
    s_face = float(np.mean(face_scores)) if face_scores else None

    exp = exposure_metrics(rgb)
    sigma = noise_sigma(gray)
    out: dict[str, float | None] = {
        "sharpness": s_face if s_face is not None else s_center,
        "sharpness_center": s_center,
        "sharpness_face": s_face,
        "laplacian_var": lap,
        "tenengrad": ten,
        "exposure": exp["exposure"],
        "mean_luminance": exp["mean_luminance"],
        "clipped_highlights": exp["clipped_highlights"],
        "crushed_shadows": exp["crushed_shadows"],
        "noise": _clamp01(sigma / NOISE_FULL_SCALE_SIGMA),
        "noise_sigma": sigma,
    }
    return {k: (None if v is None else round(float(v), 5)) for k, v in out.items()}
