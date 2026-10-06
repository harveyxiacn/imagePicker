"""Face Mesh observations (478 landmarks + head pose) on a crop around a known face box.

Used by `besttake.compose` (alignment, head-pose delta, masks) and `enhance.run face_restore`
(5-point alignment). Same crop logic as `beauty.prepare` (`FaceMesh`), but additionally returns the
head pose from MediaPipe's facial transformation matrix.
"""

from __future__ import annotations

import math
import threading
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import cv2
import numpy as np

from .beauty import landmarks as lmk
from .steps.faces import _euler_from_matrix

CROP_MARGIN = 0.35
MIN_CROP_SIDE = 256


@dataclass
class FaceObs:
    pts: np.ndarray  # (478, 2) pixels in the frame the face was observed in
    pose: tuple[float, float, float] | None  # (yaw, pitch, roll) degrees, MediaPipe convention

    @property
    def width(self) -> float:
        return float(np.ptp(self.pts[: lmk.N_FACE, 0]))

    @property
    def height(self) -> float:
        return float(np.ptp(self.pts[: lmk.N_FACE, 1]))


def wrap_pitch(p: float) -> float:
    return (p + 90.0) % 180.0 - 90.0


def pose_from_landmarks(pts: np.ndarray) -> tuple[float, float, float]:
    """Rough (yaw, pitch, roll) in degrees from 2-D landmarks (used when no pose matrix exists).

    yaw  from the nose tip's offset between the outer face edges (234 / 454),
    roll from the eye line, pitch from the nose-tip position between the eye line and the mouth.
    """
    left, right, nose = pts[234], pts[454], pts[1]
    half = max(1e-6, float(np.linalg.norm(right - left)) / 2)
    mid = (left + right) / 2
    u = (right - left) / (2 * half)
    off = float((nose - mid) @ u) / half
    yaw = math.degrees(math.asin(max(-1.0, min(1.0, off * 1.2))))
    eye_r = pts[lmk.RIGHT_EYE].mean(0)
    eye_l = pts[lmk.LEFT_EYE].mean(0)
    d = eye_l - eye_r
    roll = math.degrees(math.atan2(d[1], d[0]))
    eyes = (eye_r + eye_l) / 2
    mouth = pts[[13, 14]].mean(0)
    span = max(1e-6, float(np.linalg.norm(mouth - eyes)))
    t = float((nose - eyes) @ ((mouth - eyes) / span)) / span  # ~0.55 for a frontal face
    pitch = (t - 0.55) * 120.0
    return yaw, pitch, roll


class PoseFaceMesh:
    """MediaPipe Face Landmarker: 478 landmarks of the face nearest a given box + head pose."""

    def __init__(self, model_path: str | Path):
        from mediapipe.tasks.python import BaseOptions, vision

        opts = vision.FaceLandmarkerOptions(
            base_options=BaseOptions(model_asset_path=str(model_path)),
            running_mode=vision.RunningMode.IMAGE,
            num_faces=4,
            min_face_detection_confidence=0.3,
            min_face_presence_confidence=0.3,
            output_facial_transformation_matrixes=True,
        )
        self._lm: Any = vision.FaceLandmarker.create_from_options(opts)
        self._lock = threading.Lock()

    def observe(
        self, rgb: np.ndarray, face_px: tuple[float, float, float, float]
    ) -> FaceObs | None:
        import mediapipe as mp

        h, w = rgb.shape[:2]
        x, y, bw, bh = face_px
        cx, cy = x + bw / 2, y + bh / 2
        side = max(bw, bh) * (1 + 2 * CROP_MARGIN)
        x0, y0 = int(max(0, cx - side / 2)), int(max(0, cy - side / 2))
        x1, y1 = int(min(w, cx + side / 2)), int(min(h, cy + side / 2))
        crop = rgb[y0:y1, x0:x1]
        if crop.size == 0:
            return None
        cw, ch = crop.shape[1], crop.shape[0]
        if min(cw, ch) < MIN_CROP_SIDE:
            s = MIN_CROP_SIDE / min(cw, ch)
            crop = cv2.resize(crop, None, fx=s, fy=s, interpolation=cv2.INTER_CUBIC)
        img = mp.Image(image_format=mp.ImageFormat.SRGB, data=np.ascontiguousarray(crop))
        with self._lock:
            res = self._lm.detect(img)
        best, best_i, best_d = None, -1, 0.6 * max(bw, bh)
        for i, face in enumerate(res.face_landmarks):
            if len(face) < lmk.N_FULL:
                continue
            pts = np.array([[x0 + p.x * cw, y0 + p.y * ch] for p in face], dtype=np.float64)
            d = float(np.linalg.norm(pts[: lmk.N_FACE].mean(0) - (cx, cy)))
            if d < best_d:
                best, best_i, best_d = pts, i, d
        if best is None:
            return None
        pose = None
        mats = res.facial_transformation_matrixes
        if mats and best_i < len(mats):
            yaw, pitch, roll = _euler_from_matrix(mats[best_i])
            pose = (yaw, wrap_pitch(pitch), roll)
        return FaceObs(best, pose)

    def close(self) -> None:
        lm, self._lm = self._lm, None
        if lm is not None:
            lm.close()
