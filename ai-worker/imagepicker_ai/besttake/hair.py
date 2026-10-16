"""Hair transfer ("换成我最好的发型", doc 03 section 5.6): pure numpy / OpenCV, models injected.

The user's case: photos of the same person from one day or event; early frames have good hair,
later ones sweaty, flat, stringy or shiny hair. `hair_compose_arrays(target, donor, ...)` replaces
the target's hair with the donor's and keeps the target's own face:

  1  landmarks + head pose of both faces. Yaw / pitch / roll beyond HAIR_MAX_* -> `pose_too_different`;
     a donor face much smaller than the target face (it would be upscaled) -> `donor_too_small`.
  2  alignment: similarity transform (scale, rotation, shift) from the stable landmarks only. The two
     photos may be from different places and times, so there is no background homography as in
     Best Take; the head is the only common reference.
  3  masks (selfie multiclass on the target crop and on the aligned donor crop): hair of both, the
     target's face oval, eyes / brows / mouth. Protected: everything of the target face below the
     brows (soft fall-off outward); the forehead may receive donor hair (fringe) or donor forehead
     skin where the target's own strands hang over it.
  4  paste region = donor hair (+ forehead where the target had hair); fill region = target hair the
     donor hair does not cover (grown a little so flyaway strands go too), outside the face; filled
     from the target's surroundings (LaMa when installed, else OpenCV Telea).
  5  lighting: the per-channel LAB median difference of the two faces' skin (same person, so the
     skin difference is the light / white-balance difference) is applied to the donor; clamped.
  6  soft matte: the donor's hair probability (not a binary cut) sets the hair edge; the whole patch
     alpha is Gaussian-feathered and forced to 0 over the protected face.
  7  quality: pose delta, fill share, seam gradient -> score + warnings.

`hair_metrics(rgb, face, components)` measures the hair-quality cues donor ranking uses (doc 03
section 5.6): hair area relative to the face, sharpness inside the hair, specular shine in the hair
and on the skin, and how ragged the hair silhouette is. All at a normalised face width.
"""

from __future__ import annotations

import math
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
from . import align
from .blend import boundary_ring, seam_metrics
from .compose import Components
from .region import _ell, _keep_component, oval_of

MIN_FACE_PX = 48
# Pose gates (degrees). Hair hangs with gravity and is seen from one side: a donor turned or tilted
# further than this cannot be pasted convincingly by a 2-D similarity warp (doc 03 section 5.6).
HAIR_MAX_YAW = 20.0
HAIR_MAX_PITCH = 15.0
HAIR_MAX_ROLL = 20.0
# donor face width / target face width below this: the donor hair would be upscaled > 2x
MIN_DONOR_SCALE = 0.5
WARN_DONOR_SCALE = 0.8
MAX_LANDMARK_RMS = 0.08  # of the face width, after the similarity fit
MIN_HAIR_FRAC = 0.04  # donor hair area / face oval area below this: no hair to transfer
WARN_FILL_FRAC = 0.35  # fill area / paste area above this: a lot of background is invented
WARN_SEAM_GRAD = 0.6
LIGHT_LIM = (18.0, 8.0, 8.0)  # clamp of the skin-based LAB lighting shift (L, a, b)
METRIC_FACE_W = 160.0  # hair metrics are measured at this face width (px)

# a fill engine: (RGB crop u8, mask u8 {0, 1}) -> RGB crop with the mask filled
FillFn = Callable[[np.ndarray, np.ndarray], np.ndarray]


@dataclass
class HairOut:
    patch: np.ndarray | None  # HxWx4 uint8 RGBA, or None
    rect: tuple[int, int, int, int] | None  # x, y, w, h in target pixels
    reason: str | None = None
    quality: dict[str, Any] = field(default_factory=dict)
    align: dict[str, float] = field(default_factory=dict)
    debug: dict[str, Any] = field(default_factory=dict)


def _fail(reason: str, **debug: Any) -> HairOut:
    return HairOut(None, None, reason, {}, {}, debug)


def telea_fill(rgb: np.ndarray, mask: np.ndarray) -> np.ndarray:
    """Model-free fill: OpenCV Telea from the surroundings (radius ~ 1.5 % of the crop)."""
    r = max(3, round(0.015 * max(rgb.shape[:2])))
    return cv2.inpaint(rgb, (mask > 0).astype(np.uint8), r, cv2.INPAINT_TELEA)


def pose_of(obs: FaceObs) -> tuple[float, float, float]:
    return tuple(obs.pose) if obs.pose is not None else pose_from_landmarks(obs.pts)  # type: ignore[return-value]


def pose_delta(a: tuple[float, float, float], b: tuple[float, float, float]) -> dict[str, float]:
    return {
        "yaw": abs(a[0] - b[0]),
        "pitch": abs(wrap_pitch(a[1] - b[1])),
        "roll": abs((a[2] - b[2] + 180.0) % 360.0 - 180.0),
    }


def pose_gate(delta: dict[str, float]) -> bool:
    """True when the pose difference is small enough for a hair transfer."""
    return (
        delta["yaw"] <= HAIR_MAX_YAW
        and delta["pitch"] <= HAIR_MAX_PITCH
        and delta["roll"] <= HAIR_MAX_ROLL
    )


def head_rect(pts: np.ndarray, iw: int, ih: int) -> tuple[int, int, int, int]:
    """Crop around the head with room for hair: x0, y0, x1, y1."""
    oval = pts[lmk.FACE_OVAL]
    x0, y0 = oval.min(0)
    x1, y1 = oval.max(0)
    ow, oh = x1 - x0, y1 - y0
    cx = (x0 + x1) / 2
    cy = (y0 + y1) / 2
    return (
        int(max(0, math.floor(cx - 1.3 * ow))),
        int(max(0, math.floor(cy - 1.5 * oh))),
        int(min(iw, math.ceil(cx + 1.3 * ow))),
        int(min(ih, math.ceil(cy + 1.4 * oh))),
    )


def _hair_binary(prob: np.ndarray, oval: np.ndarray, fw: float) -> np.ndarray:
    """Binary hair mask: the components touching the head (other people's hair is dropped)."""
    hb = (prob > 0.5).astype(np.uint8)
    if not hb.any():
        return hb
    seed = cv2.dilate(oval, _ell(0.25 * fw))
    hb = _keep_component(hb, seed)
    touching = hb & seed
    if not touching.any():
        return np.zeros_like(hb)
    return hb


def _strays(prob: np.ndarray, main: np.ndarray, fw: float) -> np.ndarray:
    """`main` plus the loose hair components near it (flyaways, separate strands)."""
    raw = (prob > 0.5).astype(np.uint8)
    if not main.any():
        return main
    n, lab = cv2.connectedComponents(raw, connectivity=8)
    near = cv2.dilate(main, _ell(0.3 * fw)) > 0
    hit = np.zeros(n, bool)
    hit[np.unique(lab[near & (raw > 0)])] = True
    hit[0] = False
    return (main | hit[lab].astype(np.uint8)).astype(np.uint8)


def _skin_sel(probs: np.ndarray, oval: np.ndarray, feat: np.ndarray) -> np.ndarray:
    return (probs[..., CLS_FACE_SKIN] > 0.5) & (oval > 0) & (feat == 0)


def _lab(rgb: np.ndarray) -> np.ndarray:
    return cv2.cvtColor(rgb.astype(np.float32) / 255.0, cv2.COLOR_RGB2LAB)


def _from_lab(lab: np.ndarray) -> np.ndarray:
    return np.clip(np.rint(cv2.cvtColor(lab, cv2.COLOR_LAB2RGB) * 255.0), 0, 255).astype(np.uint8)


def light_shift(
    target: np.ndarray, donor: np.ndarray, sel_t: np.ndarray, sel_d: np.ndarray
) -> tuple[np.ndarray, bool]:
    """LAB shift that moves the donor's lighting onto the target's, from both faces' skin.

    Medians, with the brightest 15 % of each skin left out (shine is exactly what differs between
    the frames and must not drive the correction). (shift, measured?)"""
    if sel_t.sum() < 200 or sel_d.sum() < 200:
        return np.zeros(3, np.float32), False
    lt, ld = _lab(target)[sel_t], _lab(donor)[sel_d]

    def robust(v: np.ndarray) -> np.ndarray:
        cut = np.percentile(v[:, 0], 85)
        return np.median(v[v[:, 0] <= cut], axis=0)

    d = robust(lt) - robust(ld)
    lim = np.array(LIGHT_LIM, np.float32)
    return np.clip(d, -lim, lim).astype(np.float32), True


def protect_mask(
    pts: np.ndarray, shape: tuple[int, int], fw: float
) -> tuple[np.ndarray, np.ndarray]:
    """(protected uint8, forehead uint8): the target's face below the brow line (plus eyes, brows,
    mouth), and the part of the oval above it."""
    oval = oval_of(pts, shape)
    brow_y = float(min(pts[lmk.RIGHT_BROW][:, 1].min(), pts[lmk.LEFT_BROW][:, 1].min()) - 0.03 * fw)
    rows = np.arange(shape[0], dtype=np.float32)[:, None]
    below = np.broadcast_to(rows >= brow_y, shape)
    feat = lmk.feature_mask(pts, shape, fw)
    prot = ((oval > 0) & below) | (feat > 0)
    forehead = (oval > 0) & ~below & (feat == 0)
    return prot.astype(np.uint8), forehead.astype(np.uint8)


def _soft_outside(m: np.ndarray, fade: float) -> np.ndarray:
    """1 inside `m`, smooth fall-off to 0 over `fade` px outside it."""
    d = cv2.distanceTransform((m == 0).astype(np.uint8), cv2.DIST_L2, 3)
    t = np.clip(1.0 - d / max(fade, 1.0), 0.0, 1.0)
    return (t * t * (3 - 2 * t)).astype(np.float32)


def hair_compose_arrays(
    target: np.ndarray,
    donor: np.ndarray,
    t_face: Box,
    d_face: Box,
    comp: Components,
    fill: FillFn | None = None,
) -> HairOut:
    th, tw = target.shape[:2]
    dh, dw = donor.shape[:2]
    if min(t_face[2], d_face[2]) < MIN_FACE_PX:
        return _fail("face_too_small")
    if comp.parts is None:
        return _fail("hair_not_found")  # no segmentation: there is no hair mask to work with
    ot = comp.observe(target, t_face)
    od = comp.observe(donor, d_face)
    if ot is None or od is None:
        return _fail("face_not_found")
    delta = pose_delta(pose_of(ot), pose_of(od))
    if not pose_gate(delta):
        return _fail("pose_too_different", pose_delta=delta)
    fw = max(ot.width, 1.0)
    scale_ratio = od.width / fw
    if scale_ratio < MIN_DONOR_SCALE:
        return _fail("donor_too_small", scale=scale_ratio)

    # ---- 2. similarity donor -> target from the stable landmarks
    idx = align.STABLE_IDX
    M, rms = align.similarity_from_landmarks(od.pts[idx], ot.pts[idx])
    rms_n = rms / fw
    dbg: dict[str, Any] = {"pose_delta": delta, "landmark_rms": rms_n}
    if rms_n > MAX_LANDMARK_RMS:
        return _fail("face_not_aligned", **dbg)
    s = float(math.hypot(M[0, 0], M[1, 0]))
    rot = float(math.degrees(math.atan2(M[1, 0], M[0, 0])))
    al = {
        "scale": round(s, 4),
        "rotation_deg": round(rot, 3),
        "dx": round(float(M[0, 2]) / tw, 5),
        "dy": round(float(M[1, 2]) / th, 5),
        "landmark_rms": round(rms_n, 4),
    }

    x0, y0, x1, y1 = head_rect(ot.pts, tw, th)
    ch, cw = y1 - y0, x1 - x0
    if ch < 16 or cw < 16:
        return _fail("face_not_found", **dbg)
    shape = (ch, cw)
    t_crop = np.ascontiguousarray(target[y0:y1, x0:x1])
    Mc = M.copy()
    Mc[:, 2] -= (x0, y0)
    d_al = cv2.warpAffine(
        donor, Mc, (cw, ch), flags=cv2.INTER_CUBIC, borderMode=cv2.BORDER_REFLECT_101
    )
    valid = cv2.warpAffine(
        np.ones((dh, dw), np.uint8), Mc, (cw, ch), flags=cv2.INTER_NEAREST, borderValue=0
    )
    valid = cv2.erode(valid, _ell(2))
    tpts = ot.pts - (x0, y0)
    dpts = od.pts @ M[:, :2].T + M[:, 2] - (x0, y0)

    # ---- 3. masks
    probs_t = comp.parts(t_crop)
    probs_d = comp.parts(d_al)
    oval_t = oval_of(tpts, shape)
    oval_d = oval_of(dpts, shape)
    hair_t = _hair_binary(probs_t[..., CLS_HAIR], oval_t, fw)
    hair_d = _hair_binary(probs_d[..., CLS_HAIR] * valid, oval_d, fw)
    oval_area = max(float(oval_t.sum()), 1.0)
    if hair_d.sum() < MIN_HAIR_FRAC * oval_area:
        return _fail("donor_no_hair", **dbg)
    prot, forehead = protect_mask(tpts, shape, fw)

    # ---- 4. regions
    paste = (hair_d | (forehead & hair_t)) & (prot == 0)
    # loose strands near the hair and a margin around it go too (flyaways)
    cover = cv2.dilate(_strays(probs_t[..., CLS_HAIR], hair_t, fw), _ell(0.03 * fw))
    fill_m = (
        cover
        & (cv2.dilate(hair_d, _ell(0.01 * fw)) == 0)
        & (cv2.dilate(oval_t, _ell(0.02 * fw)) == 0)
        & (prot == 0)
    ).astype(np.uint8)

    # ---- 5. lighting
    feat_t = lmk.feature_mask(tpts, shape, fw)
    feat_d = lmk.feature_mask(dpts, shape, fw)
    sel_t = _skin_sel(probs_t, oval_t, feat_t)
    sel_d = _skin_sel(probs_d, oval_d, feat_d) & (valid > 0)
    shift, measured = light_shift(t_crop, d_al, sel_t, sel_d)
    d_cm = _from_lab(_lab(d_al) + shift) if measured else d_al
    dbg["light_shift"] = [round(float(v), 2) for v in shift]

    # ---- background fill where the target hair was bigger than the donor's
    base = t_crop
    if fill_m.any():
        base = (fill or telea_fill)(t_crop, fill_m)
        if base.shape != t_crop.shape:
            base = cv2.resize(base, (cw, ch), interpolation=cv2.INTER_LINEAR)

    # ---- 6. soft matte
    sig = max(1.0, 0.012 * fw)
    # hair edge from the donor's own probability (a soft matte), sharpened a little
    a_hair = smoothstep(probs_d[..., CLS_HAIR] * valid, 0.25, 0.75).astype(np.float32)
    a_hair = np.maximum(a_hair * cv2.dilate(hair_d, _ell(0.02 * fw)), forehead * hair_t)
    a_hair = cv2.GaussianBlur(a_hair, (0, 0), max(0.6, 0.3 * sig))
    src = (d_cm.astype(np.float32) * a_hair[..., None]) + base.astype(np.float32) * (
        1 - a_hair[..., None]
    )
    src_u8 = np.clip(np.rint(src), 0, 255).astype(np.uint8)
    support = (paste | fill_m).astype(np.uint8)
    support = cv2.dilate(support, _ell(round(1.5 * sig)))
    alpha = cv2.GaussianBlur(support.astype(np.float32), (0, 0), sig)
    alpha = np.clip(alpha * 1.05, 0, 1)
    alpha *= 1.0 - _soft_outside(prot, 2.5 * sig)
    alpha[prot > 0] = 0.0

    comp_img = np.clip(
        np.rint(t_crop * (1 - alpha[..., None]) + src_u8 * alpha[..., None]), 0, 255
    ).astype(np.uint8)

    # ---- 7. quality
    ring = boundary_ring((alpha > 0.5).astype(np.uint8), max(2, round(0.03 * fw))) & (prot == 0)
    seam_grad, misfit = seam_metrics(t_crop, src_u8, comp_img, ring)
    paste_area = max(float(paste.sum()), 1.0)
    fill_frac = float(fill_m.sum()) / paste_area
    warnings: list[str] = []
    if max(delta["yaw"] / HAIR_MAX_YAW, delta["pitch"] / HAIR_MAX_PITCH) > 0.6:
        warnings.append("pose_differs")
    if fill_frac > WARN_FILL_FRAC:
        warnings.append("large_fill")
    if scale_ratio < WARN_DONOR_SCALE:
        warnings.append("donor_small")
    if seam_grad > WARN_SEAM_GRAD:
        warnings.append("seam")
    if not measured:
        warnings.append("lighting_unmatched")
    f_pose = 1 - 0.5 * float(
        smoothstep(max(delta["yaw"] / HAIR_MAX_YAW, delta["pitch"] / HAIR_MAX_PITCH), 0.3, 1.0)
    )
    f_fill = 1 - 0.4 * float(smoothstep(fill_frac, 0.1, 0.8))
    f_seam = 1 - 0.4 * float(smoothstep(seam_grad, 0.2, 1.0))
    f_scale = 1 - 0.3 * float(smoothstep(1.0 - scale_ratio, 0.1, 0.5))
    score = float(np.clip(f_pose * f_fill * f_seam * f_scale, 0, 1))
    quality = {
        "score": round(score, 3),
        "aligned": bool(rms_n <= 0.03),
        "warnings": warnings,
    }
    dbg.update(
        seam_grad=seam_grad,
        ring_misfit=misfit,
        fill_frac=fill_frac,
        crop=(x0, y0, x1, y1),
        hair_t=hair_t,
        hair_d=hair_d,
        paste=paste,
        fill=fill_m,
        protect=prot,
        alpha=alpha,
        composite=comp_img,
        aligned_donor=d_cm,
    )

    ys, xs = np.nonzero(alpha > 0.004)
    if len(ys) == 0:
        return _fail("donor_no_hair", **dbg)
    py0, py1 = max(0, ys.min() - 1), min(ch, ys.max() + 2)
    px0, px1 = max(0, xs.min() - 1), min(cw, xs.max() + 2)
    a8 = np.clip(np.rint(alpha[py0:py1, px0:px1] * 255), 0, 255).astype(np.uint8)
    rgba = np.dstack([src_u8[py0:py1, px0:px1], a8])
    rect = (x0 + px0, y0 + py0, px1 - px0, py1 - py0)
    return HairOut(rgba, rect, None, quality, al, dbg)


# ------------------------------------------------------------------------------- ranking cues
def hair_metrics(rgb: np.ndarray, face: Box, comp: Components) -> dict[str, Any]:
    """Hair-quality cues of one face (doc 03 section 5.6), measured at METRIC_FACE_W.

    hair_ratio  hair area / face-oval area (flat, sweaty hair covers less)
    sharpness   mean |Laplacian| of the luma inside the (eroded) hair, 0-255 scale (definition)
    shine       share of hair pixels much brighter than the hair's median and nearly colourless
                (specular highlights of wet / greasy hair)
    skin_shine  the same on the face skin (sweat shine on forehead, nose, cheeks)
    roughness   share of the hair mask that a morphological opening removes (thin stray strands,
                stringy clumps)
    yaw / pitch / roll, face_px (face width in the photo)
    """
    if face[2] < 16:
        return {"ok": False, "reason": "face_too_small"}
    obs = comp.observe(rgb, face)
    if obs is None:
        return {"ok": False, "reason": "face_not_found"}
    yaw, pitch, roll = pose_of(obs)
    out: dict[str, Any] = {
        "ok": True,
        "yaw": round(float(yaw), 2),
        "pitch": round(float(pitch), 2),
        "roll": round(float(roll), 2),
        "face_px": round(float(obs.width), 1),
    }
    if comp.parts is None:
        out.update(ok=False, reason="hair_not_found")
        return out
    ih, iw = rgb.shape[:2]
    x0, y0, x1, y1 = head_rect(obs.pts, iw, ih)
    k = METRIC_FACE_W / max(obs.width, 1.0)
    crop = rgb[y0:y1, x0:x1]
    nw, nh = max(8, round((x1 - x0) * k)), max(8, round((y1 - y0) * k))
    crop = cv2.resize(crop, (nw, nh), interpolation=cv2.INTER_AREA if k < 1 else cv2.INTER_CUBIC)
    pts = (obs.pts - (x0, y0)) * k
    shape = (nh, nw)
    fw = METRIC_FACE_W
    probs = comp.parts(crop)
    oval = oval_of(pts, shape)
    hair = _hair_binary(probs[..., CLS_HAIR], oval, fw)
    area = float(hair.sum())
    out["hair_ratio"] = round(area / max(float(oval.sum()), 1.0), 4)
    lab = _lab(crop)
    L = lab[..., 0]
    chroma = np.hypot(lab[..., 1], lab[..., 2])
    inner = cv2.erode(hair, _ell(2)) > 0
    if inner.sum() >= 50:
        gray = cv2.cvtColor(crop, cv2.COLOR_RGB2GRAY).astype(np.float32)
        lap = np.abs(cv2.Laplacian(gray, cv2.CV_32F, ksize=3))
        out["sharpness"] = round(float(lap[inner].mean()), 3)
        med = float(np.median(L[inner]))
        spec = inner & (med + 25.0 < L) & (L > 45.0) & (chroma < 18.0)
        out["shine"] = round(float(spec.sum()) / float(inner.sum()), 4)
    else:
        out["sharpness"] = None
        out["shine"] = None
    feat = lmk.feature_mask(pts, shape, fw)
    skin = _skin_sel(probs, oval, feat)
    if skin.sum() >= 50:
        med = float(np.median(L[skin]))
        out["skin_shine"] = round(
            float((skin & (med + 15.0 < L) & (L > 60.0)).sum()) / float(skin.sum()), 4
        )
    else:
        out["skin_shine"] = None
    if area >= 50:
        opened = cv2.morphologyEx(hair, cv2.MORPH_OPEN, _ell(0.035 * fw))
        out["roughness"] = round(1.0 - float(opened.sum()) / area, 4)
    else:
        out["roughness"] = None
    return out
