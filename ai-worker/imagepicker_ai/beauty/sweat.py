"""Sweat cleanup without a donor photo (doc 03 section 7.5): `enhance.run` ops `deshine` / `flyaway`.

deshine   specular shine on the face skin and in the hair is compressed towards the local base
          brightness: base = normalised-convolution blur of the luma inside the region (skin or
          hair probability, eyes / brows / mouth left out so catchlights and lip gloss stay);
          excess above a knee (skin 6 L, hair 10 L) keeps 20 %; the chroma a highlight washed out
          is pulled back towards the local mean. Full-strength result; the patch `amount` is the
          user's strength.
flyaway   thin strands outside the smoothed hair silhouette (hair mask minus its opening by 3.5 %
          of the face width, plus loose hair pieces within 0.3 face widths) are filled from the
          surroundings (LaMa when installed, else Telea); nothing inside the face oval changes.

Both work per face on the head crop of `besttake.hair.head_rect` and return RGBA patches.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any

import cv2
import numpy as np

from ..besttake.compose import Components
from ..besttake.hair import FillFn, _hair_binary, _lab, _strays, head_rect, telea_fill
from ..besttake.region import _ell, fill_holes, oval_of
from ..imgio import Box
from ..masks.filters import smoothstep
from ..masks.nets import CLS_BODY_SKIN, CLS_FACE_SKIN, CLS_HAIR
from . import landmarks as lmk

SKIN_KNEE = 6.0  # L units above the local base where skin shine starts
HAIR_KNEE = 10.0
KEEP = 0.2  # share of the excess above the knee that is kept
HIGHLIGHT_AT = 8.0  # "highlight" for the statistics: this far above the knee
MIN_STRAY_PX = 20
MIN_STRAY_AREA = 0.001  # of the face width squared: smaller stray pieces are crumbs


@dataclass
class SweatOut:
    patch: np.ndarray | None  # HxWx4 uint8 RGBA (crop), or None
    rect: tuple[int, int, int, int] | None  # x, y, w, h in photo pixels
    reason: str | None = None
    stats: dict[str, Any] = field(default_factory=dict)


def _from_lab(lab: np.ndarray) -> np.ndarray:
    return np.clip(np.rint(cv2.cvtColor(lab, cv2.COLOR_LAB2RGB) * 255.0), 0, 255).astype(np.uint8)


def _local_mean(v: np.ndarray, w: np.ndarray, sigma: float) -> np.ndarray:
    num = cv2.GaussianBlur(v * w, (0, 0), sigma)
    den = cv2.GaussianBlur(w, (0, 0), sigma)
    return num / np.maximum(den, 1e-4)


def _crop(rgb: np.ndarray, face: Box, comp: Components):
    obs = comp.observe(rgb, face)
    if obs is None:
        return None
    ih, iw = rgb.shape[:2]
    x0, y0, x1, y1 = head_rect(obs.pts, iw, ih)
    crop = np.ascontiguousarray(rgb[y0:y1, x0:x1])
    return obs, (x0, y0, x1, y1), crop, obs.pts - (x0, y0), max(obs.width, 1.0)


def _patch(
    rgb_crop: np.ndarray, alpha: np.ndarray, origin: tuple[int, int]
) -> tuple[np.ndarray, tuple[int, int, int, int]] | None:
    ys, xs = np.nonzero(alpha > 0.004)
    if len(ys) == 0:
        return None
    py0, py1 = ys.min(), ys.max() + 1
    px0, px1 = xs.min(), xs.max() + 1
    a8 = np.clip(np.rint(alpha[py0:py1, px0:px1] * 255), 0, 255).astype(np.uint8)
    rgba = np.dstack([rgb_crop[py0:py1, px0:px1], a8])
    return rgba, (origin[0] + int(px0), origin[1] + int(py0), int(px1 - px0), int(py1 - py0))


def highlight_share(L: np.ndarray, sel: np.ndarray, knee: float, ref: float | None = None) -> float:
    """Share of `sel` brighter than the region's median luma (or `ref`) by knee + HIGHLIGHT_AT."""
    n = int(sel.sum())
    if not n:
        return 0.0
    med = float(np.median(L[sel])) if ref is None else ref
    return float(((med + knee + HIGHLIGHT_AT < L) & sel).sum()) / n


def deshine_arrays(rgb: np.ndarray, face: Box, comp: Components) -> SweatOut:
    if comp.parts is None:
        return SweatOut(None, None, "model_unavailable")
    got = _crop(rgb, face, comp)
    if got is None:
        return SweatOut(None, None, "face_not_found")
    _obs, (x0, y0, _x1, _y1), crop, pts, fw = got
    shape = crop.shape[:2]
    probs = comp.parts(crop)
    feat = lmk.feature_mask(pts, shape, fw).astype(np.float32)
    keep_out = cv2.GaussianBlur(feat, (0, 0), max(1.0, 0.01 * fw))
    lab = _lab(crop)
    L = lab[..., 0]
    out = lab.copy()
    total_w = np.zeros(shape, np.float32)
    stats: dict[str, Any] = {}
    for name, prob, knee, sig in (
        (
            "skin",
            np.clip(probs[..., CLS_FACE_SKIN] + probs[..., CLS_BODY_SKIN], 0, 1),
            SKIN_KNEE,
            0.12,
        ),
        ("hair", probs[..., CLS_HAIR], HAIR_KNEE, 0.06),
    ):
        w = (prob * (1 - keep_out)).astype(np.float32)
        sel = w > 0.5
        if sel.sum() < 50:
            stats[name] = None
            continue
        s = max(2.0, sig * fw)
        # base brightness without the highlights: a first pass, then again with the bright outliers
        # down-weighted, so a large shine patch does not lift its own base
        base = _local_mean(L, w, s)
        for _ in range(3):
            w2 = w * (1 - smoothstep(L - base, 0.5 * knee, knee)).astype(np.float32)
            base = _local_mean(L, np.maximum(w2, 1e-3 * w), s)
        ex = L - base
        new_ex = np.where(ex > knee, knee + (ex - knee) * KEEP, ex)
        dL = (new_ex - ex) * w  # <= 0: only darkens highlights
        h = smoothstep(ex, knee, knee + 20).astype(np.float32) * w
        ma = _local_mean(lab[..., 1], w2 + 1e-3 * w, s)
        mb = _local_mean(lab[..., 2], w2 + 1e-3 * w, s)
        out[..., 0] = out[..., 0] + dL
        out[..., 1] = out[..., 1] + (ma - lab[..., 1]) * 0.85 * h
        out[..., 2] = out[..., 2] + (mb - lab[..., 2]) * 0.85 * h
        total_w = np.maximum(total_w, w)
        med = float(np.median(L[sel]))
        stats[name] = {
            "highlight_before": round(highlight_share(L, sel, knee, med), 4),
            "highlight_after": round(highlight_share(out[..., 0], sel, knee, med), 4),
        }
    if not total_w.any():
        return SweatOut(None, None, "no_skin_or_hair", stats)
    res = _from_lab(out)
    alpha = np.clip(
        cv2.GaussianBlur((total_w > 0.05).astype(np.float32), (0, 0), max(1.0, 0.01 * fw)) * 1.05,
        0,
        1,
    )
    alpha *= 1 - feat  # eyes / brows / mouth: never touched
    p = _patch(res, alpha, (x0, y0))
    if p is None:
        return SweatOut(None, None, "no_skin_or_hair", stats)
    return SweatOut(p[0], p[1], None, stats)


def flyaway_arrays(
    rgb: np.ndarray, face: Box, comp: Components, fill: FillFn | None = None
) -> SweatOut:
    if comp.parts is None:
        return SweatOut(None, None, "model_unavailable")
    got = _crop(rgb, face, comp)
    if got is None:
        return SweatOut(None, None, "face_not_found")
    _obs, (x0, y0, _x1, _y1), crop, pts, fw = got
    shape = crop.shape[:2]
    prob = comp.parts(crop)[..., CLS_HAIR]
    oval = oval_of(pts, shape)
    main = _hair_binary(prob, oval, fw)
    if main.sum() < 50:
        return SweatOut(None, None, "hair_not_found")
    hair = _strays(prob, main, fw)
    body = cv2.morphologyEx(fill_holes(main), cv2.MORPH_OPEN, _ell(0.035 * fw))
    body = cv2.morphologyEx(body, cv2.MORPH_CLOSE, _ell(0.02 * fw))
    strays = (
        hair
        & (cv2.dilate(body, _ell(max(1.0, 0.008 * fw))) == 0)
        & (cv2.dilate(oval, _ell(0.02 * fw)) == 0)
    ).astype(np.uint8)
    # drop crumbs (the corners an opening shaves off a clean silhouette): strands are long
    k, lab, st, _ = cv2.connectedComponentsWithStats(strays, connectivity=8)
    big = np.zeros(k, bool)
    big[1:] = st[1:, cv2.CC_STAT_AREA] >= MIN_STRAY_AREA * fw * fw
    strays = big[lab].astype(np.uint8)
    n = int(strays.sum())
    stats = {"stray_px_before": n}
    if n < MIN_STRAY_PX:
        return SweatOut(None, None, "no_flyaways", stats)
    hole = cv2.dilate(strays, _ell(2))
    res = (fill or telea_fill)(crop, hole)
    alpha = np.clip(
        cv2.GaussianBlur(cv2.dilate(hole, _ell(1)).astype(np.float32), (0, 0), 1.0) * 1.1, 0, 1
    )
    alpha *= (oval == 0).astype(np.float32)
    after = (comp.parts(res)[..., CLS_HAIR] > 0.5) & (strays > 0)
    stats["stray_px_after"] = int(after.sum())
    p = _patch(res, alpha, (x0, y0))
    if p is None:
        return SweatOut(None, None, "no_flyaways", stats)
    return SweatOut(p[0], p[1], None, stats)
