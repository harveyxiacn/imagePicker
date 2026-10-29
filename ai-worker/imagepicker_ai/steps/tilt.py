"""Horizon / level estimation (doc 03 §3.1.1): how far the dominant straight structures
(horizon, building edges, door frames, poles) deviate from level. Runs inside the `quality` step;
the Rust port is `crates/ip-lite/src/tilt.rs` (cross-checked by `crates/ip-lite/tests/parity.rs`
on the fixtures written by `scripts/gen_lite_fixtures.py`), so every profile computes the same.

Convention: `tilt_deg` > 0 means the content is rotated clockwise (the horizon falls to the right);
rotating the photo counter-clockwise by `tilt_deg` levels it. Range +-TILT_MAX_DEG.

Method: an orientation-gated Hough transform restricted to near-axis lines.
  g        = gray (long edge ~1024, as the quality step), Gaussian sigma BLUR_SIGMA, Sobel gx / gy
  edges    = pixels with |grad| > EDGE_LO (|grad| in 1/MAG_STEPS steps, so that ties thin the
             same way in both implementations), thinned by non-maximum suppression across the edge,
             whose line direction is within TILT_MAX_DEG + GATE_DEG of horizontal (family H) or
             vertical (family V); weight ramps from 0 at EDGE_LO to 1 at EDGE_HI
  votes    = each edge pixel votes for the candidate angles theta (BIN_DEG steps over
             +-TILT_MAX_DEG) within GATE_DEG of its own gradient direction (triangular weight), at
             its line offset rho(theta) (linear split between 1 px bins); one accumulator per family
  P_f(theta) = sum over the rho peaks (3-bin window, local maxima) of the soft-thresholded line
             length: only collinear runs of about LINE_MIN_FRAC x long edge and more count, so
             short texture edges (foliage, hair, fabric) never add up
  tilt     = centroid of the top half of P = P_H + P_V within +-PEAK_HALF_BINS of its maximum
  confidence = (1 - exp(-support / (SUPPORT_SCALE x long edge)))   support: line length at the peak
             x (1 - rival / peak)        best other angle (> RIVAL_BINS away): perspective, clutter
             x per family f whose own best angle is elsewhere (converging verticals against a level
               horizon, a tilted table edge against upright door frames):
               1 - (1 - exp(-P_f(own) / (FAMILY_SCALE x long edge))) x (1 - P_f(tilt) / P_f(own))
             x credibility: the share of the line evidence at the tilt (+-CRED_HALF_BINS, the
               smoothing window) that perspective cannot explain. A line more than CENTRE_FRAC of
               the image off the centre (its rho offset) leans under perspective alone: verticals
               off the centre column converge when the camera pitches (upward-shot buildings, a
               window frame at the side), horizontals off the centre row converge under yaw (table
               edges, sills, railings). Perspective leans the two sides in opposite directions, so
               off-centre lines count only as far as both sides agree (centre + 2 x min(below,
               above)); a roll moves every line alike. Exception: a near-horizontal family whose
               longest line spans HORIZON_FRAC of the width or more counts in full (a horizon is
               the vanishing line of the ground and stays level under pitch, wherever it sits).
               One-sided evidence (a single door frame, a striped shirt, a table edge) scores 0.
Photos without long straight lines (portraits on plain backgrounds, foliage, food) get a confidence
near 0; scoring flags `tilted` only above a calibrated confidence (bench/eval-tilt.py).
"""

from __future__ import annotations

import math
from typing import NamedTuple

import cv2
import numpy as np

TILT_MAX_DEG = 15.0
GATE_DEG = 2.0
BIN_DEG = 0.25
BLUR_SIGMA = 1.0
EDGE_LO = 20.0
EDGE_HI = 60.0
MAG_STEPS = 16.0
MARGIN = 2
LINE_MIN_FRAC = 0.04
PEAK_HALF_BINS = 3
RIVAL_BINS = 4
SUPPORT_SCALE = 0.5
FAMILY_SCALE = 0.5
CENTRE_FRAC = 0.15
HORIZON_FRAC = 0.2
CRED_HALF_BINS = 1

_K = int(round(2 * TILT_MAX_DEG / BIN_DEG)) + 1
_THETA = -TILT_MAX_DEG + BIN_DEG * np.arange(_K, dtype=np.float64)
_COS = np.cos(np.radians(_THETA))
_SIN = np.sin(np.radians(_THETA))
_GATE_BINS = int(math.floor(2 * GATE_DEG / BIN_DEG)) + 1


def _accumulate(
    xc: np.ndarray,
    yc: np.ndarray,
    delta: np.ndarray,
    wgt: np.ndarray,
    vertical: bool,
    n_rho: int,
    r0: float,
) -> np.ndarray:
    """(angle bin, rho bin) accumulator of one family's edge pixels."""
    acc = np.zeros(_K * n_rho, np.float64)
    if len(delta) == 0:
        return acc.reshape(_K, n_rho)
    k0 = np.ceil((delta - GATE_DEG + TILT_MAX_DEG) / BIN_DEG).astype(np.int64)
    for j in range(_GATE_BINS + 1):
        k = k0 + j
        ok = (k >= 0) & (k < _K)
        kk = np.where(ok, k, 0)
        dist = np.abs(_THETA[kk] - delta)
        ok &= dist < GATE_DEG
        if not ok.any():
            continue
        kk, dd = kk[ok], dist[ok]
        wv = wgt[ok] * (1.0 - dd / GATE_DEG)
        c, s = _COS[kk], _SIN[kk]
        rho = (xc[ok] * c + yc[ok] * s if vertical else yc[ok] * c - xc[ok] * s) + r0
        r = np.floor(rho).astype(np.int64)
        f = rho - r
        base = kk * n_rho + r
        # only the span this pass touches: the same per-bin sums as full-length bincounts
        lo, hi = int(base.min()), int(base.max()) + 2
        acc[lo:hi] += np.bincount(base - lo, wv * (1.0 - f), minlength=hi - lo)
        acc[lo:hi] += np.bincount(base + 1 - lo, wv * f, minlength=hi - lo)
    return acc.reshape(_K, n_rho)


class Lines(NamedTuple):
    """One family's line evidence per candidate angle."""

    total: np.ndarray  # P(theta): soft-thresholded strength of the rho peaks
    below: np.ndarray  # the part from lines more than CENTRE_FRAC of the image off-centre, one side
    above: np.ndarray  # ... and the other side
    longest: np.ndarray  # strength (collinear length) of the strongest single line


def _peaks(acc: np.ndarray, line_min: float) -> tuple[np.ndarray, np.ndarray]:
    """(rho peaks' strength, their soft-thresholded value; 0 elsewhere) of each angle row."""
    a3 = acc.copy()
    a3[:, 1:] += acc[:, :-1]
    a3[:, :-1] += acc[:, 1:]
    pad = np.pad(a3, ((0, 0), (2, 2)))
    c = pad[:, 2:-2]
    peak = (c > pad[:, 1:-3]) & (c > pad[:, :-4]) & (c >= pad[:, 3:-1]) & (c >= pad[:, 4:])
    soft = np.clip((c - line_min) / line_min, 0.0, 1.0)
    return np.where(peak, c, 0.0), np.where(peak, c * soft, 0.0)


def _line_profile(acc: np.ndarray, line_min: float) -> np.ndarray:
    """P(theta): soft-thresholded strength of the rho peaks of each angle row."""
    return _peaks(acc, line_min)[1].sum(axis=1)


def _lines(acc: np.ndarray, line_min: float, r0: int, edge: float) -> Lines:
    """Line evidence of one family's accumulator; rho bin j is a line j - r0 px off the centre."""
    c, val = _peaks(acc, line_min)
    off = np.arange(acc.shape[1]) - r0
    return Lines(
        val.sum(axis=1),
        val[:, off < -edge].sum(axis=1),
        val[:, off > edge].sum(axis=1),
        c.max(axis=1),
    )


def line_evidence(gray: np.ndarray) -> tuple[Lines, Lines]:
    """Line evidence (H, V) of the near-horizontal / near-vertical lines of a uint8 image."""
    h, w = gray.shape
    if min(h, w) < 4 * MARGIN + 16:
        z = np.zeros(_K)
        return Lines(z, z, z, z), Lines(z, z, z, z)
    g = cv2.GaussianBlur(gray.astype(np.float32), (0, 0), BLUR_SIGMA)
    gx = cv2.Sobel(g, cv2.CV_32F, 1, 0, ksize=3).astype(np.float64)
    gy = cv2.Sobel(g, cv2.CV_32F, 0, 1, ksize=3).astype(np.float64)
    # 1/16 steps: equal magnitudes on both sides of a pixel-aligned edge stay equal whatever the
    # float rounding, so the thinning keeps the same pixel here and in the Rust port
    mag = np.round(np.sqrt(gx * gx + gy * gy) * MAG_STEPS) / MAG_STEPS
    t = math.tan(math.radians(TILT_MAX_DEG + GATE_DEG))
    strong = mag > EDGE_LO
    strong[:MARGIN] = strong[-MARGIN:] = False
    strong[:, :MARGIN] = strong[:, -MARGIN:] = False
    # family gate and non-maximum suppression across the edge (up/down for H, left/right for V),
    # evaluated on the strong pixels only: the margin keeps the neighbours inside the image, and
    # the pixels stay in row-major order (as np.nonzero of full-image masks), so every sum below
    # adds the same numbers in the same order
    ys0, xs0 = np.nonzero(strong)
    m0, gx0, gy0 = mag[ys0, xs0], gx[ys0, xs0], gy[ys0, xs0]
    ax, ay = np.abs(gx0), np.abs(gy0)
    hsel = (ax <= t * ay) & (m0 >= mag[ys0 - 1, xs0]) & (m0 > mag[ys0 + 1, xs0])
    vsel = (ay <= t * ax) & (m0 >= mag[ys0, xs0 - 1]) & (m0 > mag[ys0, xs0 + 1])
    line_min = LINE_MIN_FRAC * max(w, h)
    r0 = math.ceil(math.hypot(w, h) / 2) + 2
    n_rho = 2 * r0 + 2
    cx, cy = (w - 1) / 2.0, (h - 1) / 2.0
    out = []
    for sel, vertical in ((hsel, False), (vsel, True)):
        ys, xs = ys0[sel], xs0[sel]
        gxs, gys = gx0[sel], gy0[sel]
        delta = np.degrees(np.arctan(gys / gxs if vertical else -gxs / gys))
        wgt = np.clip((m0[sel] - EDGE_LO) / (EDGE_HI - EDGE_LO), 0.0, 1.0)
        acc = _accumulate(xs - cx, ys - cy, delta, wgt, vertical, n_rho, r0)
        # offsets across the lines: vertical for H, horizontal for V
        out.append(_lines(acc, line_min, r0, CENTRE_FRAC * (w if vertical else h)))
    return out[0], out[1]


def line_profiles(gray: np.ndarray) -> tuple[np.ndarray, np.ndarray]:
    """Angle profiles (P_H, P_V) of the near-horizontal / near-vertical lines of a uint8 image."""
    lh, lv = line_evidence(gray)
    return lh.total, lv.total


def _smooth(p: np.ndarray) -> np.ndarray:
    sm = p.copy()
    sm[1:] += p[:-1]
    sm[:-1] += p[1:]
    return sm


def credibility(lh: Lines, lv: Lines, k: int, width: int) -> float:
    """Share of the line evidence at angle bin k that perspective cannot explain (module doc)."""
    lo, hi = max(0, k - CRED_HALF_BINS), min(_K, k + CRED_HALF_BINS + 1)
    total = credible = 0.0
    for fam, ln in ((0, lh), (1, lv)):
        t = float(ln.total[lo:hi].sum())
        total += t
        if fam == 0 and float(ln.longest[lo:hi].max()) >= HORIZON_FRAC * width:
            credible += t
        else:
            below, above = float(ln.below[lo:hi].sum()), float(ln.above[lo:hi].sum())
            credible += t - below - above + 2.0 * min(below, above)
    return credible / total if total > 0 else 0.0


def combine(lh: Lines, lv: Lines, w: int, h: int) -> tuple[float, float]:
    """(tilt_deg, confidence) from the two families' line evidence of a w x h image."""
    prof_h, prof_v = lh.total, lv.total
    long_edge = max(w, h)
    prof = prof_h + prof_v
    if prof.sum() <= 0:
        return 0.0, 0.0
    sm = _smooth(prof)
    k = int(np.argmax(sm))
    lo, hi = max(0, k - PEAK_HALF_BINS), min(_K, k + PEAK_HALF_BINS + 1)
    win = prof[lo:hi]
    top = np.maximum(win - 0.5 * win.max(), 0.0)
    tilt = float((top * _THETA[lo:hi]).sum() / top.sum())
    far = np.abs(np.arange(_K) - k) > RIVAL_BINS
    dominance = max(0.0, 1.0 - float(sm[far].max()) / float(sm[k]))
    conf = (1.0 - math.exp(-float(win.max()) / (SUPPORT_SCALE * long_edge))) * dominance
    for p in (prof_h, prof_v):
        smf = _smooth(p)
        kf = int(np.argmax(smf))
        own = float(smf[kf])
        if abs(kf - k) > RIVAL_BINS and own > 0:
            strength = 1.0 - math.exp(-own / (FAMILY_SCALE * long_edge))
            conf *= 1.0 - strength * (1.0 - float(smf[k]) / own)
    conf *= credibility(lh, lv, k, w)
    return round(tilt, 2), round(conf, 3)


def estimate_tilt(gray: np.ndarray) -> tuple[float, float]:
    """(tilt_deg, confidence 0..1) of a uint8 gray image at analysis size (long edge ~1024)."""
    lh, lv = line_evidence(gray)
    return combine(lh, lv, gray.shape[1], gray.shape[0])
