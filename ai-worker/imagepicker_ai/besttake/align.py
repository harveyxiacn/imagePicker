"""Frame alignment for Best Take (doc 03 section 5, steps 4-5).

Global:  ORB (AKAZE fallback) features + RANSAC homography source -> base on an analysis-size copy
         (faces excluded: they move), refined with ECC (homography) on a 1/2-scale copy. Rejected
         (`camera_moved`) when too few inliers, a large residual, an implausible transform (shift /
         scale / rotation) or a low correlation of the aligned backgrounds.
Local:   similarity transform from stable face landmarks (jaw line, nose, forehead) mapping the
         source face onto the base face, blended into the global homography outside the face
         (`face_weight`), plus a dense DIS optical-flow refinement inside the face region that is
         held back around the eyes / brows / mouth (those differ on purpose) and clamped to a
         small fraction of the face width.
"""

from __future__ import annotations

import math
from dataclasses import dataclass

import cv2
import numpy as np

from ..beauty import landmarks as lmk

ANALYSIS_SIDE = 1280
MIN_INLIERS = 14
MIN_INLIER_FRAC = 0.2
MAX_RESIDUAL_PX = 3.0  # median inlier error at analysis scale
MAX_SHIFT_FRAC = 0.12  # of the long edge
MAX_SCALE_DEV = 0.15
MAX_ROT_DEG = 8.0
MIN_BG_NCC = 0.55

# landmark subset that moves rigidly with the head (no eyes / brows / mouth)
# the chin arc is left out: it moves with the jaw (open mouth, laughing)
_CHIN_ARC = {397, 365, 379, 378, 400, 377, 152, 148, 176, 149, 150, 136}
STABLE_IDX = sorted(
    (set(lmk.FACE_OVAL) - _CHIN_ARC)
    | {1, 2, 4, 5, 6, 8, 9, 19, 94, 97, 98, 129, 168, 195, 197, 358}
)


@dataclass
class GlobalAlign:
    ok: bool
    H: np.ndarray  # 3x3 source -> base, full-resolution pixel coordinates
    inliers: int = 0
    matches: int = 0
    residual_px: float = 0.0  # median inlier error at analysis scale
    shift_frac: float = 0.0
    scale: float = 1.0
    rot_deg: float = 0.0
    ncc: float = 0.0
    reason: str | None = None


def _gray_small(rgb: np.ndarray, side: int) -> tuple[np.ndarray, float]:
    s = min(1.0, side / max(rgb.shape[:2]))
    g = cv2.cvtColor(rgb, cv2.COLOR_RGB2GRAY)
    if s < 1.0:
        g = cv2.resize(g, None, fx=s, fy=s, interpolation=cv2.INTER_AREA)
    return g, s


def _exclusion_mask(
    shape: tuple[int, int], boxes: list[tuple[float, float, float, float]], s: float
):
    m = np.full(shape, 255, np.uint8)
    for x, y, w, h in boxes:
        cx, cy = (x + w / 2) * s, (y + h / 2) * s
        hw, hh = w * s * 1.3, h * s * 1.5  # generous: head, hair, neck move with the face
        cv2.rectangle(
            m,
            (int(cx - hw), int(cy - hh * 0.8)),
            (int(cx + hw), int(cy + hh * 1.4)),
            0,
            -1,
        )
    return m


def _akaze():
    for name in ("AKAZE_create", "AKAZE"):  # opencv 4.x / 5.x spellings
        f = getattr(cv2, name, None)
        if f is not None:
            return f.create() if hasattr(f, "create") else f()
    return None


def _features(gray: np.ndarray, mask: np.ndarray, kind: str):
    if kind == "orb":
        det = cv2.ORB_create(nfeatures=4000, fastThreshold=10)
        norm = cv2.NORM_HAMMING
    else:
        det = _akaze()
        norm = cv2.NORM_HAMMING
    if det is None:
        return [], None, norm
    kps, des = det.detectAndCompute(gray, mask)
    return kps, des, norm


def _match(kind: str, g_src, m_src, g_base, m_base):
    ks, ds, norm = _features(g_src, m_src, kind)
    kb, db, _ = _features(g_base, m_base, kind)
    if ds is None or db is None or len(ks) < 8 or len(kb) < 8:
        return None
    bf = cv2.BFMatcher(norm)
    pairs = bf.knnMatch(ds, db, k=2)
    good = [p[0] for p in pairs if len(p) == 2 and p[0].distance < 0.75 * p[1].distance]
    if len(good) < 8:
        return None
    src = np.float32([ks[m.queryIdx].pt for m in good])
    dst = np.float32([kb[m.trainIdx].pt for m in good])
    return src, dst


def decompose(H: np.ndarray, w: int, h: int) -> tuple[float, float, float]:
    """(shift of the image centre as a fraction of the long edge, scale, rotation deg)."""
    c = np.array([[[w / 2, h / 2]]], np.float32)
    c2 = cv2.perspectiveTransform(c, H)[0, 0]
    shift = float(np.hypot(c2[0] - w / 2, c2[1] - h / 2)) / max(w, h)
    # local affine at the centre via finite differences
    d = 50.0
    pts = np.array([[[w / 2, h / 2]], [[w / 2 + d, h / 2]], [[w / 2, h / 2 + d]]], np.float32)
    q = cv2.perspectiveTransform(pts, H)[:, 0]
    ex, ey = (q[1] - q[0]) / d, (q[2] - q[0]) / d
    scale = math.sqrt(abs(ex[0] * ey[1] - ex[1] * ey[0]))
    rot = math.degrees(math.atan2(ex[1], ex[0]))
    return shift, scale, rot


def align_global(
    base: np.ndarray,
    src: np.ndarray,
    base_faces: list[tuple[float, float, float, float]],
    src_faces: list[tuple[float, float, float, float]],
) -> GlobalAlign:
    """Homography source -> base (full-res px). `*_faces`: face boxes (px) to keep out of matching."""
    bh, bw = base.shape[:2]
    sh, sw = src.shape[:2]
    gb, sb = _gray_small(base, ANALYSIS_SIDE)
    gs, ss = _gray_small(src, ANALYSIS_SIDE)
    mb = _exclusion_mask(gb.shape, base_faces, sb)
    ms = _exclusion_mask(gs.shape, src_faces, ss)

    best = None
    for kind in ("orb", "akaze"):
        mt = _match(kind, gs, ms, gb, mb)
        if mt is None:
            continue
        src_pts, dst_pts = mt
        H, inl = cv2.findHomography(
            src_pts, dst_pts, cv2.RANSAC, 3.0, maxIters=5000, confidence=0.995
        )
        if H is None:
            continue
        n_in = int(inl.sum())
        if best is None or n_in > best[2]:
            best = (H, inl.ravel().astype(bool), n_in, len(src_pts), src_pts, dst_pts)
        if n_in >= 60 and n_in / len(src_pts) > 0.4:
            break

    def fail(reason: str, **kw) -> GlobalAlign:
        return GlobalAlign(False, np.eye(3), reason=reason, **kw)

    if best is None:
        return fail("camera_moved")
    H_a, inl, n_in, n_m, p_s, p_b = best
    if n_in < MIN_INLIERS or n_in / n_m < MIN_INLIER_FRAC:
        return fail("camera_moved", inliers=n_in, matches=n_m)

    # ECC refinement on a half-scale copy (analysis coordinates)
    H_ref = H_a
    try:
        k = 0.5
        gb2 = (
            cv2.resize(gb, None, fx=k, fy=k, interpolation=cv2.INTER_AREA).astype(np.float32) / 255
        )
        gs2 = (
            cv2.resize(gs, None, fx=k, fy=k, interpolation=cv2.INTER_AREA).astype(np.float32) / 255
        )
        mb2 = cv2.resize(mb, (gb2.shape[1], gb2.shape[0]), interpolation=cv2.INTER_NEAREST)
        Sk = np.diag([k, k, 1.0])
        # ECC warps the *input* (source) onto the template (base): warp maps template -> input
        # coordinates, i.e. the inverse of our source -> base transform.
        init = (Sk @ np.linalg.inv(H_a) @ np.linalg.inv(Sk)).astype(np.float32)
        gb2b = cv2.GaussianBlur(gb2, (0, 0), 1.0)
        gs2b = cv2.GaussianBlur(gs2, (0, 0), 1.0)
        cc, W = cv2.findTransformECC(
            gb2b,
            gs2b,
            init,
            cv2.MOTION_HOMOGRAPHY,
            (cv2.TERM_CRITERIA_EPS | cv2.TERM_CRITERIA_COUNT, 60, 1e-5),
            mb2,
            5,
        )
        cand = np.linalg.inv(Sk) @ np.linalg.inv(W) @ Sk
        cand /= cand[2, 2]
        corners = np.float32(
            [[0, 0], [gb.shape[1], 0], [gb.shape[1], gb.shape[0]], [0, gb.shape[0]]]
        )
        d = np.linalg.norm(
            cv2.perspectiveTransform(corners[None], cand)[0]
            - cv2.perspectiveTransform(corners[None], H_a)[0],
            axis=1,
        ).max()
        if cc > 0.5 and d < 0.04 * max(gb.shape):
            H_ref = cand
    except cv2.error:
        pass

    # residual on the RANSAC inliers (analysis scale) and background NCC after alignment
    proj = cv2.perspectiveTransform(p_s[inl][None].astype(np.float32), H_ref)[0]
    residual = float(np.median(np.linalg.norm(proj - p_b[inl], axis=1)))
    shift, scale, rot = decompose(H_ref, gb.shape[1], gb.shape[0])
    ncc = _bg_ncc(gb, gs, H_ref, mb)

    # analysis -> full-res coordinates
    Sb = np.diag([1 / sb, 1 / sb, 1.0])
    Ss = np.diag([ss, ss, 1.0])
    H_full = Sb @ H_ref @ Ss
    H_full /= H_full[2, 2]
    info = dict(
        inliers=n_in,
        matches=n_m,
        residual_px=residual,
        shift_frac=shift,
        scale=scale,
        rot_deg=rot,
        ncc=ncc,
    )
    if (
        shift > MAX_SHIFT_FRAC
        or abs(scale - 1) > MAX_SCALE_DEV
        or abs(rot) > MAX_ROT_DEG
        or residual > MAX_RESIDUAL_PX
        or ncc < MIN_BG_NCC
    ):
        return GlobalAlign(False, H_full, reason="camera_moved", **info)
    return GlobalAlign(True, H_full, **info)


def _bg_ncc(gb: np.ndarray, gs: np.ndarray, H: np.ndarray, mask: np.ndarray) -> float:
    """Normalised correlation of the (blurred) aligned backgrounds over the common valid area."""
    h, w = gb.shape
    warped = cv2.warpPerspective(gs, H, (w, h), flags=cv2.INTER_LINEAR)
    valid = cv2.warpPerspective(np.full(gs.shape, 255, np.uint8), H, (w, h)) > 250
    valid &= mask > 0
    valid = cv2.erode(valid.astype(np.uint8), np.ones((5, 5), np.uint8)).astype(bool)
    if valid.sum() < 1000:
        return 0.0
    a = cv2.GaussianBlur(gb, (0, 0), 1.5).astype(np.float32)[valid]
    b = cv2.GaussianBlur(warped, (0, 0), 1.5).astype(np.float32)[valid]
    a -= a.mean()
    b -= b.mean()
    den = float(np.sqrt((a * a).sum() * (b * b).sum()))
    return float((a * b).sum() / den) if den > 1e-6 else 0.0


# ---------------------------------------------------------------------------------------- local
def similarity_from_landmarks(
    pts_src_in_base: np.ndarray, pts_base: np.ndarray
) -> tuple[np.ndarray, float]:
    """Similarity (2x3) mapping `pts_src_in_base` onto `pts_base` (stable landmarks) + RMS error.

    Robust (LMedS) so a few landmarks displaced by hair / hands do not tilt the fit.
    """
    a = np.asarray(pts_src_in_base, np.float32)
    b = np.asarray(pts_base, np.float32)
    M, _ = cv2.estimateAffinePartial2D(a, b, method=cv2.LMEDS)
    if M is None:
        M = np.array([[1, 0, float((b - a)[:, 0].mean())], [0, 1, float((b - a)[:, 1].mean())]])
    m = M.astype(np.float64)
    res = a @ m[:, :2].T + m[:, 2] - b
    return m, float(np.sqrt((res**2).sum(1).mean()))


def face_weight(oval_mask: np.ndarray, fade_px: float) -> np.ndarray:
    """1 inside the face oval (+ small margin), smooth fall-off to 0 over `fade_px` outside."""
    inv = (oval_mask == 0).astype(np.uint8)
    d = cv2.distanceTransform(inv, cv2.DIST_L2, 3)
    t = np.clip(1.0 - d / max(fade_px, 1.0), 0.0, 1.0)
    return (t * t * (3 - 2 * t)).astype(np.float32)


def refine_flow(
    base_gray: np.ndarray,
    src_gray: np.ndarray,
    weight: np.ndarray,
    exclude: np.ndarray,
    face_w: float,
) -> np.ndarray:
    """DIS flow base -> aligned source, restricted: (h, w, 2) float32 displacement in px.

    `weight` (0..1) fades the refinement out away from the face; `exclude` (uint8, 1 = expression
    region such as eyes / brows / mouth) is filled by normalised convolution from its surroundings
    so the open eye is never "pulled" onto the closed one. |flow| is clamped to 3 % of the face
    width and smoothed.
    """
    h, w = base_gray.shape
    s = min(1.0, 640.0 / max(h, w))  # everything below runs at <= 640 px: sub-2 px residuals
    sh, sw = max(1, round(h * s)), max(1, round(w * s))
    if s < 1.0:
        bg = cv2.resize(base_gray, (sw, sh), interpolation=cv2.INTER_AREA)
        sg = cv2.resize(src_gray, (sw, sh), interpolation=cv2.INTER_AREA)
        wt = cv2.resize(weight, (sw, sh), interpolation=cv2.INTER_AREA)
        ex = cv2.resize(exclude.astype(np.float32), (sw, sh), interpolation=cv2.INTER_AREA)
    else:
        bg, sg, wt, ex = base_gray, src_gray, weight, exclude.astype(np.float32)
    dis = cv2.DISOpticalFlow_create(cv2.DISOPTICAL_FLOW_PRESET_MEDIUM)
    flow = dis.calc(bg, sg, None)
    valid = (1 - ex).clip(0, 1) * wt
    fw = face_w * s
    sigma = max(2.0, 0.12 * fw)
    num = cv2.GaussianBlur(flow * valid[..., None], (0, 0), sigma)
    den = cv2.GaussianBlur(valid, (0, 0), sigma)[..., None]
    fill = num / np.maximum(den, 1e-4)
    keep = valid[..., None]
    flow = flow * keep + fill * (1 - keep)
    flow = cv2.GaussianBlur(flow, (0, 0), max(1.0, 0.02 * fw))
    mag = np.linalg.norm(flow, axis=2, keepdims=True)
    flow = flow * np.minimum(1.0, 0.03 * fw / np.maximum(mag, 1e-6))
    flow = (flow * wt[..., None]).astype(np.float32)
    if s < 1.0:
        flow = cv2.resize(flow, (w, h), interpolation=cv2.INTER_LINEAR) / s
    return flow.astype(np.float32)
