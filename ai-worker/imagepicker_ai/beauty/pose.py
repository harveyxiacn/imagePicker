"""MediaPipe Pose Landmarker (33 keypoints) on a body crop + association with a face box."""

from __future__ import annotations

import threading
from pathlib import Path
from typing import Any

import cv2
import numpy as np

# face-ish pose keypoints: nose, eyes (inner/centre/outer x2), ears, mouth corners
HEAD_POINTS = (0, 2, 5, 7, 8, 9, 10)
MIN_VIS = 0.3
MAX_HEAD_DIST = 0.6  # head-centre distance to the face-box centre, in face sizes
MIN_CROP_SIDE = 384

# crop window around a face (in face widths / heights): wide enough for outstretched arms
CROP_HALF_WIDTH = 3.2
CROP_ABOVE = 1.2
CROP_BELOW = 8.0


def pose_crop(face_px: tuple[float, float, float, float], iw: int, ih: int) -> tuple[int, ...]:
    """(x0, y0, x1, y1) body crop for a face box (pixels), clipped to the image."""
    x, y, w, h = face_px
    cx = x + w / 2
    x0 = max(0, int(np.floor(cx - CROP_HALF_WIDTH * w)))
    x1 = min(iw, int(np.ceil(cx + CROP_HALF_WIDTH * w)))
    y0 = max(0, int(np.floor(y - CROP_ABOVE * h)))
    y1 = min(ih, int(np.ceil(y + (1.0 + CROP_BELOW) * h)))
    return x0, y0, max(x0 + 1, x1), max(y0 + 1, y1)


# MediaPipe pose limb / torso connections used as watershed seeds
SKELETON = (
    (11, 12), (11, 13), (13, 15), (12, 14), (14, 16), (11, 23), (12, 24), (23, 24),
    (23, 25), (25, 27), (24, 26), (26, 28), (15, 19), (16, 20),
)  # fmt: skip
SEED_VIS = 0.5


def skeleton_marker(
    pose: list[list[float]], shape: tuple[int, int], thickness_px: float
) -> np.ndarray | None:
    """uint8 {0, 1} mask of the visible pose limbs as thick polylines (None if nothing visible)."""
    if len(pose) < 33:
        return None
    h, w = shape
    m = np.zeros(shape, np.uint8)
    t = max(1, round(thickness_px))
    drawn = False
    for a, b in SKELETON:
        pa, pb = pose[a], pose[b]
        if pa[2] < SEED_VIS or pb[2] < SEED_VIS:
            continue
        if not all(-0.05 <= v <= 1.05 for v in (pa[0], pa[1], pb[0], pb[1])):
            continue
        cv2.line(
            m, (round(pa[0] * w), round(pa[1] * h)), (round(pb[0] * w), round(pb[1] * h)), 1, t
        )
        drawn = True
    return m if drawn else None


def pick_pose(
    poses: list[list[list[float]]], face_box: tuple[float, float, float, float]
) -> list[list[float]]:
    """The pose whose head keypoints sit on `face_box` (normalised [x, y, w, h]); [] if none.

    Score = distance between the mean of the visible head keypoints (nose, eyes, ears, mouth)
    and the box centre, in units of the box size; the best pose under MAX_HEAD_DIST wins.
    The nose must additionally be inside the box grown by 35 % when it is visible.
    """
    fx, fy, fw, fh = face_box
    c = np.array([fx + fw / 2, fy + fh / 2])
    size = max(fw, fh)
    best, best_d = None, MAX_HEAD_DIST
    for p in poses:
        a = np.asarray(p, dtype=np.float64)
        if a.shape[0] < 33:
            continue
        idx = [i for i in HEAD_POINTS if a[i, 2] >= MIN_VIS]
        if not idx:
            continue
        if a[0, 2] >= MIN_VIS:
            nx, ny = a[0, 0], a[0, 1]
            if not (
                fx - 0.35 * fw <= nx <= fx + 1.35 * fw and fy - 0.35 * fh <= ny <= fy + 1.35 * fh
            ):
                continue
        d = float(np.linalg.norm(a[idx, :2].mean(0) - c) / size)
        if d < best_d:
            best, best_d = p, d
    return best if best is not None else []


class PoseEstimator:
    """Thread-safe wrapper (calls are serialised) around `mediapipe.tasks...PoseLandmarker`."""

    def __init__(self, model_path: str | Path, max_poses: int = 4):
        from mediapipe.tasks.python import BaseOptions, vision

        opts = vision.PoseLandmarkerOptions(
            base_options=BaseOptions(model_asset_path=str(model_path)),
            running_mode=vision.RunningMode.IMAGE,
            num_poses=max_poses,
            min_pose_detection_confidence=0.3,
            min_pose_presence_confidence=0.3,
            min_tracking_confidence=0.3,
        )
        self._lm: Any = vision.PoseLandmarker.create_from_options(opts)
        self._lock = threading.Lock()
        self.model_id = "mediapipe-pose-landmarker-full"

    def detect(self, rgb: np.ndarray) -> list[list[list[float]]]:
        """All poses in `rgb` as [[x, y, visibility] * 33] in crop-normalised coordinates."""
        import mediapipe as mp

        h, w = rgb.shape[:2]
        if min(h, w) < MIN_CROP_SIDE:
            s = MIN_CROP_SIDE / min(h, w)
            rgb = cv2.resize(rgb, None, fx=s, fy=s, interpolation=cv2.INTER_CUBIC)
        img = mp.Image(image_format=mp.ImageFormat.SRGB, data=np.ascontiguousarray(rgb))
        with self._lock:
            res = self._lm.detect(img)
        return [
            [[float(p.x), float(p.y), float(p.visibility or 0.0)] for p in pose]
            for pose in res.pose_landmarks
        ]

    def pose_for_face(
        self, rgb: np.ndarray, face_px: tuple[float, float, float, float]
    ) -> list[list[float]]:
        """33 keypoints `[x, y, visibility]` (normalised to the full image) of the face's body."""
        ih, iw = rgb.shape[:2]
        x0, y0, x1, y1 = pose_crop(face_px, iw, ih)
        cw, ch = x1 - x0, y1 - y0
        poses = self.detect(rgb[y0:y1, x0:x1])
        # to full-image normalised coordinates
        full = [
            [[(x0 + p[0] * cw) / iw, (y0 + p[1] * ch) / ih, p[2]] for p in pose] for pose in poses
        ]
        fb = (face_px[0] / iw, face_px[1] / ih, face_px[2] / iw, face_px[3] / ih)
        return pick_pose(full, fb)

    def close(self) -> None:
        lm, self._lm = self._lm, None
        if lm is not None:
            lm.close()
