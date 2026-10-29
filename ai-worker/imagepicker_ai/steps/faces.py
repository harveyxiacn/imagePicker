"""Face step: YuNet detection (OpenCV) + MediaPipe Face Landmarker blendshapes / head pose.

Per face the result carries
  bbox [x, y, w, h] normalized to the analysis image, det_score,
  eyes_open = 1 - max(eyeBlinkLeft, eyeBlinkRight)
  smile     = mean(mouthSmileLeft, mouthSmileRight)
  yaw / pitch / roll (degrees, from the facial transformation matrix; sign follows MediaPipe's
  camera-space convention, magnitude is what the scorer uses)
  gaze      0-1 "looking at the camera": iris position inside the eye socket (MediaPipe iris
            landmarks vs eye corners / lids) combined with head yaw / pitch, see `gaze_score`
  mouth_open  jawOpen blendshape (0 closed ... 1 wide open; speech is about 0.05-0.2)
  bad_expression  None, "talking" or "half_blink", see `expression_flag`
  sharpness (0-1, see quality.face_sharpness), blendshapes {52 coefficients}
  artificial  0-1 confidence that the face is on a statue / poster / screen / print (steps/liveness.py)
  likely_artificial  artificial >= liveness.ARTIFICIAL_AT; such faces never become bystanders
If mediapipe is not installed (or no landmarks were found for a face) the landmark-derived
fields are null and `landmarks` is false.
"""

from __future__ import annotations

import logging
import math
import threading
from pathlib import Path
from typing import Any

import cv2
import numpy as np

from .liveness import ARTIFICIAL_AT, face_artificial, image_saturation
from .quality import face_sharpness

# MediaPipe Face Landmarker topology (468 face points + 10 iris points)
_EYES = (
    # (corner A (image left), corner B (image right), upper lid, lower lid, iris centre)
    (33, 133, 159, 145, 468),  # subject's right eye (image left)
    (362, 263, 386, 374, 473),  # subject's left eye (image right)
)
GAZE_SIGMA_H = 0.11  # iris offset along the eye axis, in eye widths (0 = centred)
GAZE_SIGMA_V = 0.10  # iris offset across the eye axis, in eye widths
GAZE_SIGMA_YAW = 25.0  # degrees
GAZE_SIGMA_PITCH = 20.0

# "Expression not great" (docs/03 §3.3), a tag only: a subject mid-word or caught half-blinking.
# MediaPipe's jawOpen stays small for speech: on the library's non-smiling subjects mid-sentence
# p25 0.04 / p50 0.11, a closed mouth p90 0.02 / p95 0.06 (bench/eval-bystander.py).
MOUTH_OPEN_TALKING = 0.06  # jawOpen at or above this ...
EXPR_SMILE_BELOW = 0.35  # ... without a smile is "talking" (a laugh or a smile is fine)
HALF_BLINK = (0.45, 0.65)  # eyes_open in [lo, hi) is half shut; below lo is the closed_eyes tag
# Landmarks on a face this blurry (quality.face_sharpness; the blurry-tag threshold) are guesses:
# no expression tag for it.
EXPR_MIN_SHARPNESS = 0.35

log = logging.getLogger(__name__)

CROP_MARGIN = 0.35  # extra context around the YuNet box fed to the landmarker
MIN_CROP_SIDE = 256


def mediapipe_available() -> bool:
    try:
        import mediapipe  # noqa: F401

        return True
    except Exception:  # noqa: BLE001
        return False


def _euler_from_matrix(m: np.ndarray) -> tuple[float, float, float]:
    """(yaw, pitch, roll) degrees from a 4x4 (or 3x3) pose matrix; ZYX decomposition."""
    r = np.asarray(m, dtype=np.float64)[:3, :3]
    # strip any uniform scale
    cols = np.linalg.norm(r, axis=0)
    r = r / np.where(cols > 1e-9, cols, 1.0)
    sy = math.hypot(r[0, 0], r[1, 0])
    if sy > 1e-6:
        pitch = math.atan2(r[2, 1], r[2, 2])
        yaw = math.atan2(-r[2, 0], sy)
        roll = math.atan2(r[1, 0], r[0, 0])
    else:  # gimbal lock
        pitch = math.atan2(-r[1, 2], r[1, 1])
        yaw = math.atan2(-r[2, 0], sy)
        roll = 0.0
    return math.degrees(yaw), math.degrees(pitch), math.degrees(roll)


def iris_offsets(pts: np.ndarray) -> tuple[float, float] | None:
    """Mean iris offset from the eye centre in eye widths: (along eye axis, across it).

    `pts` are the 478 MediaPipe landmarks in pixels. Returns None for degenerate eyes.
    """
    dxs, dys = [], []
    for a, b, up, lo, iris in _EYES:
        pa, pb, pu, pl, pi_ = pts[a], pts[b], pts[up], pts[lo], pts[iris]
        axis = pb - pa
        width = float(np.linalg.norm(axis))
        if width < 4:
            return None
        u = axis / width
        n = np.array([-u[1], u[0]])
        centre = (pa + pb) / 2 * 0.5 + (pu + pl) / 2 * 0.5  # corners + lids average
        d = pi_ - centre
        dxs.append(float(d @ u) / width)
        dys.append(float(d @ n) / width)
    return float(np.mean(dxs)), float(np.mean(dys))


def gaze_score(
    pts: np.ndarray, yaw: float, pitch: float, eyes_open: float | None = None
) -> float | None:
    """0-1 "looking at the camera".

    score = pose * eye * lids
      pose = exp(-0.5 * ((yaw/25 deg)^2 + (pitch/20 deg)^2))      head turned away -> low
      eye  = exp(-0.5 * ((dx/0.11)^2 + (dy/0.10)^2))              iris off-centre in the socket -> low
      lids = smoothstep(eyes_open, 0.25, 0.6)                    (closed eyes are not looking)
    Head rotation compensated by opposite eye rotation is deliberately not credited (conservative).
    """
    off = iris_offsets(pts)
    if off is None:
        return None
    dx, dy = off
    # pitch from the pose matrix may be reported around 180 deg for some conventions: wrap to +-90
    p = (pitch + 90.0) % 180.0 - 90.0
    pose = math.exp(-0.5 * ((yaw / GAZE_SIGMA_YAW) ** 2 + (p / GAZE_SIGMA_PITCH) ** 2))
    eye = math.exp(-0.5 * ((dx / GAZE_SIGMA_H) ** 2 + (dy / GAZE_SIGMA_V) ** 2))
    lids = 1.0
    if eyes_open is not None:
        t = min(1.0, max(0.0, (eyes_open - 0.25) / 0.35))
        lids = t * t * (3 - 2 * t)
    return float(min(1.0, max(0.0, pose * eye * lids)))


def expression_flag(
    eyes_open: float | None,
    smile: float | None,
    mouth_open: float | None,
    sharpness: float | None = None,
) -> str | None:
    """`"talking"`, `"half_blink"` or None. Mirrors `scoring::bad_expression` in ip-core.

    talking     mouth_open >= 0.06 and smile < 0.35
    half_blink  0.45 <= eyes_open < 0.65 and smile < 0.35
    Never for a face with sharpness < 0.35 (blurred landmarks). Needs the landmarker (all inputs
    None without it -> None).
    """
    if smile is None or smile >= EXPR_SMILE_BELOW:
        return None
    if sharpness is not None and sharpness < EXPR_MIN_SHARPNESS:
        return None
    if mouth_open is not None and mouth_open >= MOUTH_OPEN_TALKING:
        return "talking"
    if eyes_open is not None and HALF_BLINK[0] <= eyes_open < HALF_BLINK[1]:
        return "half_blink"
    return None


class FaceAnalyzer:
    """Thread-safe: detector / landmarker instances are created lazily per thread."""

    def __init__(
        self,
        yunet_path: str | Path,
        landmarker_path: str | Path | None = None,
        score_threshold: float = 0.7,
        max_faces: int = 30,
    ):
        self.yunet_path = str(yunet_path)
        self.landmarker_path = str(landmarker_path) if landmarker_path else None
        self.score_threshold = score_threshold
        self.max_faces = max_faces
        self._tls = threading.local()
        self._landmarker_ok = bool(self.landmarker_path) and mediapipe_available()
        if self.landmarker_path and not self._landmarker_ok:
            log.warning(
                "mediapipe not available: faces step runs YuNet-only (landmark fields null)"
            )

    @property
    def has_landmarks(self) -> bool:
        return self._landmarker_ok

    # ---- per-thread resources
    def _detector(self) -> Any:
        d = getattr(self._tls, "det", None)
        if d is None:
            d = cv2.FaceDetectorYN.create(
                self.yunet_path, "", (320, 320), self.score_threshold, 0.3, 5000
            )
            self._tls.det = d
        return d

    def _landmarker(self) -> Any:
        lm = getattr(self._tls, "lm", None)
        if lm is None:
            from mediapipe.tasks.python import BaseOptions, vision

            opts = vision.FaceLandmarkerOptions(
                base_options=BaseOptions(model_asset_path=self.landmarker_path),
                running_mode=vision.RunningMode.IMAGE,
                num_faces=1,
                min_face_detection_confidence=0.3,
                min_face_presence_confidence=0.3,
                output_face_blendshapes=True,
                output_facial_transformation_matrixes=True,
            )
            lm = vision.FaceLandmarker.create_from_options(opts)
            self._tls.lm = lm
        return lm

    # ---- API
    def detect(self, rgb: np.ndarray) -> list[dict[str, Any]]:
        h, w = rgb.shape[:2]
        det = self._detector()
        det.setInputSize((w, h))
        _, rows = det.detect(cv2.cvtColor(rgb, cv2.COLOR_RGB2BGR))
        faces: list[dict[str, Any]] = []
        if rows is None:
            return faces
        for r in rows[: self.max_faces]:
            x, y, bw, bh = (float(v) for v in r[:4])
            x0, y0 = max(0.0, x), max(0.0, y)
            x1, y1 = min(float(w), x + bw), min(float(h), y + bh)
            if x1 - x0 < 4 or y1 - y0 < 4:
                continue
            faces.append(
                {
                    # YuNet 5-point landmarks (px in the analysed image): eye (image left),
                    # eye (image right), nose, mouth corner (left), mouth corner (right).
                    # Private: consumed by the identity step and stripped from the result.
                    "_kps": [[float(r[4 + 2 * i]), float(r[5 + 2 * i])] for i in range(5)],
                    "bbox": [
                        round(x0 / w, 5),
                        round(y0 / h, 5),
                        round((x1 - x0) / w, 5),
                        round((y1 - y0) / h, 5),
                    ],
                    "det_score": round(float(r[14]), 4),
                }
            )
        faces.sort(key=lambda f: -f["bbox"][2] * f["bbox"][3])
        return faces

    def analyze(self, rgb: np.ndarray) -> list[dict[str, Any]]:
        faces = self.detect(rgb)
        image_sat = image_saturation(rgb) if faces else None
        for f in faces:
            f["sharpness"] = _r(face_sharpness(rgb, tuple(f["bbox"])))  # type: ignore[arg-type]
            art = face_artificial(rgb, tuple(f["bbox"]), image_sat)  # type: ignore[arg-type]
            f["artificial"] = _r(art)
            f["likely_artificial"] = art is not None and art >= ARTIFICIAL_AT
            f.update(_NULL_LANDMARK_FIELDS)
            if self._landmarker_ok:
                try:
                    f.update(self._landmark(rgb, f["bbox"]))
                except Exception as e:  # noqa: BLE001
                    log.debug("landmarker failed: %s", e)
            f["bad_expression"] = expression_flag(
                f["eyes_open"], f["smile"], f["mouth_open"], f["sharpness"]
            )
        return faces

    def _landmark(self, rgb: np.ndarray, bbox: list[float]) -> dict[str, Any]:
        import mediapipe as mp

        h, w = rgb.shape[:2]
        x, y, bw, bh = bbox[0] * w, bbox[1] * h, bbox[2] * w, bbox[3] * h
        cx, cy = x + bw / 2, y + bh / 2
        side = max(bw, bh) * (1 + 2 * CROP_MARGIN)
        x0, y0 = int(max(0, cx - side / 2)), int(max(0, cy - side / 2))
        x1, y1 = int(min(w, cx + side / 2)), int(min(h, cy + side / 2))
        crop = rgb[y0:y1, x0:x1]
        if min(crop.shape[:2]) < MIN_CROP_SIDE:
            s = MIN_CROP_SIDE / min(crop.shape[:2])
            crop = cv2.resize(crop, None, fx=s, fy=s, interpolation=cv2.INTER_CUBIC)
        img = mp.Image(image_format=mp.ImageFormat.SRGB, data=np.ascontiguousarray(crop))
        res = self._landmarker().detect(img)
        if not res.face_blendshapes:
            return {}
        bs = {c.category_name: float(c.score) for c in res.face_blendshapes[0]}
        out: dict[str, Any] = {
            "landmarks": True,
            "eyes_open": _r(1.0 - max(bs.get("eyeBlinkLeft", 0.0), bs.get("eyeBlinkRight", 0.0))),
            "smile": _r((bs.get("mouthSmileLeft", 0.0) + bs.get("mouthSmileRight", 0.0)) / 2),
            "mouth_open": _r(bs.get("jawOpen", 0.0)),
            "blendshapes": {k: round(v, 4) for k, v in bs.items() if k != "_neutral"},
        }
        if res.facial_transformation_matrixes:
            yaw, pitch, roll = _euler_from_matrix(res.facial_transformation_matrixes[0])
            out.update(yaw=round(yaw, 2), pitch=round(pitch, 2), roll=round(roll, 2))
        if res.face_landmarks and out.get("yaw") is not None:
            lm = res.face_landmarks[0]
            if len(lm) >= 478:
                pts = np.array([[p.x * crop.shape[1], p.y * crop.shape[0]] for p in lm])
                out["gaze"] = _r(gaze_score(pts, out["yaw"], out["pitch"], out["eyes_open"]))
        return out

    def close(self) -> None:
        # per-thread landmarkers are released with their threads; drop our reference
        self._tls = threading.local()


_NULL_LANDMARK_FIELDS: dict[str, Any] = {
    "landmarks": False,
    "eyes_open": None,
    "smile": None,
    "mouth_open": None,
    "yaw": None,
    "pitch": None,
    "roll": None,
    "blendshapes": None,
    "gaze": None,
}


def _r(v: float | None) -> float | None:
    return None if v is None else round(float(v), 4)
