"""Best Take compositing core (doc 03 section 5): pure numpy / OpenCV, models injected.

`compose_arrays(base, source, base_face, source_face, components)` implements the whole pipeline
on two upright full-resolution RGB frames and returns the RGBA patch (see `ComposeOut`):

  1  landmarks (478) + head pose of both faces; head-pose delta > 25 deg  -> `large_pose_change`
  2  global alignment source -> base (ORB/AKAZE + RANSAC, ECC)            -> `camera_moved`
  3  local alignment: landmark similarity inside the face, global homography outside, DIS flow
  4  blend region: face + hair + neck of both frames, matte-restricted, other faces cut out
  5  LAB colour match on the boundary band, 6 Laplacian-pyramid blend
  7  quality: seam discontinuity, landmark error after the warp, occlusion -> score + warnings
"""

from __future__ import annotations

import logging
import math
import time
from collections.abc import Callable
from dataclasses import dataclass, field
from typing import Any

import cv2
import numpy as np

from ..beauty import landmarks as lmk
from ..facemesh import FaceObs, pose_from_landmarks, wrap_pitch
from ..imgio import Box
from ..masks.filters import smoothstep
from ..masks.nets import CLS_FACE_SKIN, CLS_HAIR
from ..masks.person import body_crop
from . import align
from .blend import (
    boundary_ring,
    color_match,
    multiband_blend,
    pyramid_levels,
    seam_metrics,
)
from .region import _ell, build_region, oval_of

log = logging.getLogger(__name__)

MIN_FACE_PX = 48  # smaller faces cannot be composited convincingly
MAX_POSE_DELTA = 25.0  # degrees: beyond this the candidate is not composable
WARN_POSE_DELTA = 15.0
MAX_LANDMARK_ERR = 0.10  # of the face width, after the warp: beyond this -> not composable
WARN_LANDMARK_ERR = 0.03
WARN_SEAM_GRAD = 0.6
WARN_RING_MISFIT = 0.08
MIN_SKIN_FRAC = 0.5


@dataclass
class Components:
    """Everything model-dependent; each piece but `observe` is optional (graceful degradation)."""

    observe: Callable[[np.ndarray, Box], FaceObs | None]
    # RGB crop -> HxWx6 selfie-multiclass probabilities at the crop's size
    parts: Callable[[np.ndarray], np.ndarray] | None = None
    # (RGB window, face box in window, other face boxes in window) -> HxW person matte in [0, 1]
    matte: Callable[[np.ndarray, Box, list[Box]], np.ndarray] | None = None
    # full RGB frame -> face boxes (px)
    detect_faces: Callable[[np.ndarray], list[Box]] | None = None


@dataclass
class ComposeOut:
    patch: np.ndarray | None  # HxWx4 uint8 RGBA, or None when not composable
    rect: tuple[int, int, int, int] | None  # x, y, w, h in base pixels
    reason: str | None = None
    quality: dict[str, Any] = field(default_factory=dict)
    debug: dict[str, Any] = field(default_factory=dict)


def _fail(reason: str, **debug: Any) -> ComposeOut:
    return ComposeOut(None, None, reason, {}, debug)


def _same_face(a: Box, b: Box) -> bool:
    ax, ay = a[0] + a[2] / 2, a[1] + a[3] / 2
    bx, by = b[0] + b[2] / 2, b[1] + b[3] / 2
    return abs(ax - bx) < 0.5 * max(a[2], b[2]) and abs(ay - by) < 0.5 * max(a[3], b[3])


def apply_h(M: np.ndarray, x: np.ndarray, y: np.ndarray) -> tuple[np.ndarray, np.ndarray]:
    """Apply a 3x3 projective matrix to coordinate grids."""
    d = M[2, 0] * x + M[2, 1] * y + M[2, 2]
    return (
        ((M[0, 0] * x + M[0, 1] * y + M[0, 2]) / d).astype(np.float32),
        ((M[1, 0] * x + M[1, 1] * y + M[1, 2]) / d).astype(np.float32),
    )


def _pts_h(M: np.ndarray, pts: np.ndarray) -> np.ndarray:
    return cv2.perspectiveTransform(np.asarray(pts, np.float64)[None], M)[0]


def _crop_rect(pts: np.ndarray, iw: int, ih: int) -> tuple[int, int, int, int]:
    oval = pts[lmk.FACE_OVAL]
    x0, y0 = oval.min(0)
    x1, y1 = oval.max(0)
    ow, oh = x1 - x0, y1 - y0
    cx = (x0 + x1) / 2
    cy = (y0 + y1) / 2
    cx0 = int(max(0, math.floor(cx - 1.0 * ow)))
    cx1 = int(min(iw, math.ceil(cx + 1.0 * ow)))
    cy0 = int(max(0, math.floor(cy - 1.1 * oh)))
    cy1 = int(min(ih, math.ceil(cy + 1.0 * oh)))
    return cx0, cy0, cx1, cy1


def _matte_window(
    rgb: np.ndarray, face: Box, others: list[Box], fn: Callable[..., np.ndarray]
) -> tuple[np.ndarray, int, int]:
    ih, iw = rgb.shape[:2]
    x0, y0, x1, y1 = body_crop(face, iw, ih)
    sub = np.ascontiguousarray(rgb[y0:y1, x0:x1])
    loc: Box = (face[0] - x0, face[1] - y0, face[2], face[3])
    oth = [(o[0] - x0, o[1] - y0, o[2], o[3]) for o in others]
    return fn(sub, loc, oth).astype(np.float32), x0, y0


def _face_box_from_pts(pts: np.ndarray) -> Box:
    x0, y0 = pts[: lmk.N_FACE].min(0)
    x1, y1 = pts[: lmk.N_FACE].max(0)
    return (float(x0), float(y0), float(x1 - x0), float(y1 - y0))


def _boxes_mask(shape: tuple[int, int], boxes: list[Box], grow: float = 1.25) -> np.ndarray:
    m = np.zeros(shape, np.uint8)
    for x, y, w, h in boxes:
        c = (int(round(x + w / 2)), int(round(y + h / 2)))
        cv2.ellipse(m, c, (int(w * grow / 2 * 1.1), int(h * grow / 2 * 1.2)), 0, 0, 360, 1, -1)
    return m.astype(np.float32)


class _Timer:
    def __init__(self) -> None:
        self.t = time.perf_counter()
        self.marks: dict[str, float] = {}

    def mark(self, name: str) -> None:
        now = time.perf_counter()
        self.marks[name] = round(now - self.t, 4)
        self.t = now


def compose_arrays(
    base: np.ndarray,
    src: np.ndarray,
    base_face: Box,
    src_face: Box,
    comp: Components,
) -> ComposeOut:
    bh, bw = base.shape[:2]
    sh, sw = src.shape[:2]

    tm = _Timer()
    # ---- 1. landmarks + head pose
    if min(base_face[2], src_face[2]) < MIN_FACE_PX:
        return _fail("face_too_small")
    ob = comp.observe(base, base_face)
    os_ = comp.observe(src, src_face)
    if ob is None or os_ is None:
        return _fail("face_not_found")
    pb = ob.pose or pose_from_landmarks(ob.pts)
    ps = os_.pose or pose_from_landmarks(os_.pts)
    dyaw = abs(pb[0] - ps[0])
    dpitch = abs(wrap_pitch(pb[1] - ps[1]))
    pose_delta = math.hypot(dyaw, dpitch)
    if pose_delta > MAX_POSE_DELTA:
        return _fail("large_pose_change", pose_delta=pose_delta)

    # ---- other people (kept out of the matching and out of the blend region)
    others_b: list[Box] = []
    others_s: list[Box] = []
    if comp.detect_faces is not None:
        others_b = [b for b in comp.detect_faces(base) if not _same_face(b, base_face)]
        others_s = [b for b in comp.detect_faces(src) if not _same_face(b, src_face)]

    tm.mark("landmarks")
    # ---- 2. global alignment
    ga = align.align_global(
        base,
        src,
        [base_face, *others_b],
        [src_face, *others_s],
    )
    tm.mark("global_align")
    dbg: dict[str, Any] = {"global": ga, "pose_delta": pose_delta, "timings": tm.marks}
    if not ga.ok:
        return _fail("camera_moved", **dbg)
    H = ga.H

    # ---- 3. local alignment
    idx = align.STABLE_IDX
    q_s = _pts_h(H, os_.pts[idx])
    sim, lm_rms_pre = align.similarity_from_landmarks(q_s, ob.pts[idx])
    sim3 = np.vstack([sim, [0, 0, 1]])
    Tf = sim3 @ H  # source -> base (face)
    Tf_inv = np.linalg.inv(Tf)
    H_inv = np.linalg.inv(H)
    src_pts_in_base = _pts_h(Tf, os_.pts)
    face_w = ob.width
    dbg["landmark_rms_pre"] = lm_rms_pre / max(face_w, 1.0)

    cx0, cy0, cx1, cy1 = _crop_rect(ob.pts, bw, bh)
    ch, cw = cy1 - cy0, cx1 - cx0
    if ch < 16 or cw < 16:
        return _fail("face_not_found", **dbg)
    base_crop = np.ascontiguousarray(base[cy0:cy1, cx0:cx1])
    gy, gx = np.mgrid[0:ch, 0:cw].astype(np.float32)
    gxb, gyb = gx + cx0, gy + cy0  # base full-res coordinates of the crop pixels

    bpts_c = ob.pts - (cx0, cy0)
    spts_c = src_pts_in_base - (cx0, cy0)
    shape = (ch, cw)
    oval_c = oval_of(bpts_c, shape) | oval_of(spts_c, shape)
    wface = align.face_weight(oval_c, 0.45 * face_w)

    mfx, mfy = apply_h(Tf_inv, gxb, gyb)
    mhx, mhy = apply_h(H_inv, gxb, gyb)
    m0x = wface * mfx + (1 - wface) * mhx
    m0y = wface * mfy + (1 - wface) * mhy
    s0 = cv2.remap(src, m0x, m0y, cv2.INTER_CUBIC, borderMode=cv2.BORDER_REFLECT_101)

    # DIS flow refinement, held back around the expression features
    kf = 0.5  # the expression mask only gates the flow / the misfit: half resolution is plenty
    fshape = (max(1, round(ch * kf)), max(1, round(cw * kf)))
    feat_s = np.zeros(fshape, np.uint8)
    for pts in (bpts_c, spts_c):
        for ids in (lmk.RIGHT_EYE, lmk.LEFT_EYE, lmk.RIGHT_BROW, lmk.LEFT_BROW, lmk.LIPS_OUTER):
            feat_s |= lmk.polygon_mask(pts * kf, ids, fshape, 0.05 * face_w * kf)
    feat = cv2.resize(feat_s, (cw, ch), interpolation=cv2.INTER_NEAREST)
    gray_b = cv2.cvtColor(base_crop, cv2.COLOR_RGB2GRAY)
    gray_s = cv2.cvtColor(s0, cv2.COLOR_RGB2GRAY)
    flow = align.refine_flow(gray_b, gray_s, wface, feat, face_w)
    m_x = cv2.remap(
        m0x, gx + flow[..., 0], gy + flow[..., 1], cv2.INTER_LINEAR, borderMode=cv2.BORDER_REPLICATE
    )
    m_y = cv2.remap(
        m0y, gx + flow[..., 0], gy + flow[..., 1], cv2.INTER_LINEAR, borderMode=cv2.BORDER_REPLICATE
    )
    s1 = cv2.remap(src, m_x, m_y, cv2.INTER_CUBIC, borderMode=cv2.BORDER_REFLECT_101)

    def misfit(a: np.ndarray, b: np.ndarray) -> float:
        sel = (oval_c > 0) & (feat == 0)
        if sel.sum() < 50:
            return 0.0
        d = np.abs(
            cv2.GaussianBlur(cv2.cvtColor(a, cv2.COLOR_RGB2GRAY), (0, 0), 1.0).astype(np.float32)
            - cv2.GaussianBlur(cv2.cvtColor(b, cv2.COLOR_RGB2GRAY), (0, 0), 1.0).astype(np.float32)
        )
        return float(d[sel].mean())

    e0, e1 = misfit(base_crop, s0), misfit(base_crop, s1)
    use_flow = e1 < e0 * 0.98
    s_al = s1 if use_flow else s0
    mx_f, my_f = (m_x, m_y) if use_flow else (m0x, m0y)
    dbg["flow_used"] = use_flow
    dbg["misfit_before_after_flow"] = (e0, e1)

    tm.mark("local_align")
    # ---- 4. blend region
    probs_b = probs_s = matte_b = matte_s = None
    if comp.parts is not None:
        probs_b = comp.parts(base_crop)
        probs_s = comp.parts(s_al)
    if comp.matte is not None:
        mw, wx0, wy0 = _matte_window(base, base_face, others_b, comp.matte)
        matte_b = np.zeros(shape, np.float32)
        ys0, xs0 = max(cy0, wy0), max(cx0, wx0)
        ys1, xs1 = min(cy1, wy0 + mw.shape[0]), min(cx1, wx0 + mw.shape[1])
        if ys1 > ys0 and xs1 > xs0:
            matte_b[ys0 - cy0 : ys1 - cy0, xs0 - cx0 : xs1 - cx0] = mw[
                ys0 - wy0 : ys1 - wy0, xs0 - wx0 : xs1 - wx0
            ]
        ms, sx0, sy0 = _matte_window(src, src_face, others_s, comp.matte)
        matte_s = cv2.remap(
            ms,
            mx_f - sx0,
            my_f - sy0,
            cv2.INTER_LINEAR,
            borderMode=cv2.BORDER_CONSTANT,
            borderValue=0,
        )
    others_mask = _boxes_mask(shape, [(b[0] - cx0, b[1] - cy0, b[2], b[3]) for b in others_b])
    for b in others_s:  # the source's bystanders, mapped into base coordinates
        q = _pts_h(H, np.array([[b[0], b[1]], [b[0] + b[2], b[1] + b[3]]]))
        others_mask = np.maximum(
            others_mask,
            _boxes_mask(
                shape, [(q[0, 0] - cx0, q[0, 1] - cy0, q[1, 0] - q[0, 0], q[1, 1] - q[0, 1])]
            ),
        )
    tm.mark("parts_matte")
    reg = build_region(bpts_c, spts_c, shape, probs_b, probs_s, matte_b, matte_s, others_mask)
    fw = reg.face_w
    if int(reg.binary.sum()) < 0.2 * fw * reg.face_h:
        return _fail("face_not_aligned", **dbg)

    tm.mark("region")
    # ---- 5. colour match on the boundary band
    ring_r = max(3, round(0.06 * fw))
    ring = boundary_ring(reg.binary, ring_r) & (others_mask < 0.3)
    s_cm, ring_de = color_match(base_crop, s_al, ring, 0.25 * fw)
    dbg["ring_dE"] = ring_de

    tm.mark("color_match")
    # ---- 6. multi-band blend
    sigma_f = max(1.5, 0.045 * fw)
    a_mb = cv2.GaussianBlur(reg.binary.astype(np.float32), (0, 0), sigma_f)
    levels = pyramid_levels(fw, shape)
    comp_img = multiband_blend(base_crop, s_cm, a_mb, levels)

    # patch alpha: opaque wherever the composite differs from the base, feathered outward
    changed = (np.abs(comp_img.astype(np.int16) - base_crop).max(axis=2) >= 2).astype(np.uint8)
    support = (reg.binary | changed).astype(np.uint8)
    sig_o = max(1.5, 0.03 * fw)
    support = cv2.dilate(support, _ell(round(2.0 * sig_o)))
    alpha = cv2.GaussianBlur(support.astype(np.float32), (0, 0), sig_o)
    alpha = np.clip(alpha * 1.04, 0, 1)
    cut = ((others_mask > 0.3) & (oval_c == 0)).astype(np.float32)
    alpha *= 1.0 - cv2.GaussianBlur(cut, (0, 0), sig_o * 0.5)

    tm.mark("blend")
    # ---- 7. quality
    ring_q = boundary_ring(reg.binary, max(2, round(0.03 * fw))) & (others_mask < 0.3)
    seam_grad, ring_misfit = seam_metrics(base_crop, s_cm, comp_img, ring_q)
    lm_err = _landmark_error(comp, s_al, ob, (cx0, cy0), base_face, face_w)
    if lm_err is None:
        lm_err = dbg["landmark_rms_pre"]
    occl, skin_frac, other_overlap = _occlusion(
        probs_s, spts_c, shape, others_mask, os_, sw, sh, feat
    )
    dbg["other_overlap"] = other_overlap
    dbg.update(
        seam_grad=seam_grad,
        ring_misfit=ring_misfit,
        landmark_err=lm_err,
        skin_frac=skin_frac,
        region=reg,
        aligned_source=s_al,
        composite=comp_img,
        base_crop=base_crop,
        crop=(cx0, cy0, cx1, cy1),
        levels=levels,
    )
    tm.mark("quality")
    if skin_frac < 0.3 or dbg.get("other_overlap", 0.0) > 0.3:
        return _fail("face_occluded", **dbg)
    if lm_err > MAX_LANDMARK_ERR:
        return _fail("face_not_aligned", **dbg)

    warnings: list[str] = []
    if pose_delta > WARN_POSE_DELTA:
        warnings.append("large_pose_change")
    if occl:
        warnings.append("occlusion")
    if seam_grad > WARN_SEAM_GRAD or ring_misfit > WARN_RING_MISFIT or lm_err > WARN_LANDMARK_ERR:
        warnings.append("seam")
    f_pose = 1 - 0.5 * float(smoothstep(pose_delta, 8.0, MAX_POSE_DELTA))
    f_align = 1 - 0.6 * float(smoothstep(lm_err, 0.015, MAX_LANDMARK_ERR))
    f_seam = 1 - 0.5 * float(smoothstep(max(seam_grad / 2, ring_misfit * 4), 0.1, 0.6))
    f_cam = 1 - 0.4 * float(smoothstep(ga.residual_px, 1.5, align.MAX_RESIDUAL_PX))
    f_occ = 0.6 if occl else 1.0
    score = float(np.clip(f_pose * f_align * f_seam * f_cam * f_occ, 0, 1))
    quality = {
        "score": round(score, 3),
        "aligned": bool(lm_err <= WARN_LANDMARK_ERR and ring_misfit <= WARN_RING_MISFIT),
        "warnings": warnings,
    }

    # ---- patch = RGBA crop around the alpha support
    ys, xs = np.nonzero(alpha > 0.004)
    if len(ys) == 0:
        return _fail("face_not_aligned", **dbg)
    py0, py1 = max(0, ys.min() - 1), min(ch, ys.max() + 2)
    px0, px1 = max(0, xs.min() - 1), min(cw, xs.max() + 2)
    a8 = np.clip(np.rint(alpha[py0:py1, px0:px1] * 255), 0, 255).astype(np.uint8)
    rgba = np.dstack([comp_img[py0:py1, px0:px1], a8])
    rect = (cx0 + px0, cy0 + py0, px1 - px0, py1 - py0)
    return ComposeOut(rgba, rect, None, quality, dbg)


def _landmark_error(
    comp: Components,
    s_al: np.ndarray,
    ob: FaceObs,
    origin: tuple[int, int],
    base_face: Box,
    face_w: float,
) -> float | None:
    """Landmarks of the aligned source (re-detected) vs the base ones, stable points, / face width."""
    box = (base_face[0] - origin[0], base_face[1] - origin[1], base_face[2], base_face[3])
    try:
        o2 = comp.observe(s_al, box)
    except Exception:  # noqa: BLE001
        return None
    if o2 is None:
        return None
    idx = align.STABLE_IDX
    d = np.linalg.norm(o2.pts[idx] - (ob.pts[idx] - origin), axis=1)
    return float(np.sqrt((d**2).mean()) / max(face_w, 1.0))


def _occlusion(
    probs_s: np.ndarray | None,
    spts_c: np.ndarray,
    shape: tuple[int, int],
    others: np.ndarray,
    os_: FaceObs,
    sw: int,
    sh: int,
    feat: np.ndarray,
) -> tuple[bool, float, float]:
    """(occluded?, skin fraction of the source oval, other-face overlap) for the aligned source face."""
    oval = oval_of(spts_c, shape)
    area = float(oval.sum())
    if area < 1:
        return True, 0.0, 0.0
    # oval partly outside the source frame
    o = os_.pts[lmk.FACE_OVAL]
    outside = float(
        ((o[:, 0] < -2) | (o[:, 0] > sw + 2) | (o[:, 1] < -2) | (o[:, 1] > sh + 2)).mean()
    )
    # another face over the oval
    overlap = float(((others > 0.3) & (oval > 0)).sum()) / area
    skin_frac = 1.0
    if probs_s is not None:
        sel = (oval > 0) & (feat == 0)
        if sel.sum() > 50:
            skin = np.clip(probs_s[..., CLS_FACE_SKIN] + probs_s[..., CLS_HAIR] * 0.0, 0, 1)
            skin_frac = float(skin[sel].mean())
    return (outside > 0.05 or overlap > 0.05 or skin_frac < MIN_SKIN_FRAC), skin_frac, overlap
