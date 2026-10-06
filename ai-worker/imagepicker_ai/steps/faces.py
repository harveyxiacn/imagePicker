"""Face step: YuNet detection (OpenCV) + MediaPipe Face Landmarker blendshapes / head pose.

Per face the result carries
  bbox [x, y, w, h] normalized to the analysis image, det_score,
  eyes_open = 1 - max(eyeBlinkLeft, eyeBlinkRight)
  smile     = mean(mouthSmileLeft, mouthSmileRight)
  yaw / pitch / roll (degrees, from the facial transformation matrix; sign follows MediaPipe's
  camera-space convention, magnitude is what the scorer uses)
  sharpness (0-1, see quality.face_sharpness), blendshapes {52 coefficients}
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

from .quality import face_sharpness

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
        for f in faces:
            f["sharpness"] = _r(face_sharpness(rgb, tuple(f["bbox"])))  # type: ignore[arg-type]
            f.update(_NULL_LANDMARK_FIELDS)
            if self._landmarker_ok:
                try:
                    f.update(self._landmark(rgb, f["bbox"]))
                except Exception as e:  # noqa: BLE001
                    log.debug("landmarker failed: %s", e)
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
            "blendshapes": {k: round(v, 4) for k, v in bs.items() if k != "_neutral"},
        }
        if res.facial_transformation_matrixes:
            yaw, pitch, roll = _euler_from_matrix(res.facial_transformation_matrixes[0])
            out.update(yaw=round(yaw, 2), pitch=round(pitch, 2), roll=round(roll, 2))
        return out

    def close(self) -> None:
        # per-thread landmarkers are released with their threads; drop our reference
        self._tls = threading.local()


_NULL_LANDMARK_FIELDS: dict[str, Any] = {
    "landmarks": False,
    "eyes_open": None,
    "smile": None,
    "yaw": None,
    "pitch": None,
    "roll": None,
    "blendshapes": None,
}


def _r(v: float | None) -> float | None:
    return None if v is None else round(float(v), 4)
