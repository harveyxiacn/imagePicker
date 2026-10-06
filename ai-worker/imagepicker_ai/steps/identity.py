"""Identity step: AuraFace (ArcFace ResNet-100, Apache-2.0) 512-d face embeddings.

Pipeline per face:  5 landmarks (YuNet) --similarity transform--> ArcFace 112x112 template
--> RGB, (x - 127.5) / 127.5 --> `glintr100.onnx` --> 512-d --> L2 normalise.
Cosine similarity of two embeddings is the identity score (same person ~ >0.4-0.5 for AuraFace,
see tests/test_identity.py for the measured gap).
"""

from __future__ import annotations

import logging
import threading
from pathlib import Path
from typing import Any

import cv2
import numpy as np

from ..hw import HardwareInfo
from ..models.manager import ModelManager
from ..models.registry import ModelSpec
from .embed import _make_ort_session

log = logging.getLogger(__name__)

MODEL_ID = "auraface"
IDENTITY_DIM = 512
FACE_SIZE = 112

# Standard ArcFace / insightface 112x112 landmark template:
# left eye, right eye (image side), nose tip, left mouth corner, right mouth corner.
ARCFACE_TEMPLATE = np.array(
    [
        [38.2946, 51.6963],
        [73.5318, 51.5014],
        [56.0252, 71.7366],
        [41.5493, 92.3655],
        [70.7299, 92.2041],
    ],
    dtype=np.float64,
)


def estimate_similarity(src: np.ndarray, dst: np.ndarray = ARCFACE_TEMPLATE) -> np.ndarray:
    """Least-squares similarity transform (rotation + uniform scale + translation), Umeyama.

    src, dst: (N, 2). Returns the 2x3 matrix M with  dst ~= M @ [x, y, 1]^T.
    """
    src = np.asarray(src, dtype=np.float64)
    dst = np.asarray(dst, dtype=np.float64)
    if src.shape != dst.shape or src.ndim != 2 or src.shape[1] != 2 or len(src) < 2:
        raise ValueError("need matching (N>=2, 2) point sets")
    n = len(src)
    mu_s, mu_d = src.mean(0), dst.mean(0)
    sc, dc = src - mu_s, dst - mu_d
    var_s = (sc**2).sum() / n
    if var_s < 1e-12:
        raise ValueError("degenerate landmarks")
    cov = dc.T @ sc / n
    u, d, vt = np.linalg.svd(cov)
    s = np.ones(2)
    if np.linalg.det(u) * np.linalg.det(vt) < 0:
        s[-1] = -1  # forbid reflections
    r = u @ np.diag(s) @ vt
    scale = float((d * s).sum() / var_s)
    t = mu_d - scale * (r @ mu_s)
    m = np.zeros((2, 3))
    m[:, :2] = scale * r
    m[:, 2] = t
    return m


def align_face(rgb: np.ndarray, kps: np.ndarray, size: int = FACE_SIZE) -> np.ndarray:
    """Warp a face to the 112x112 ArcFace template. rgb: HxWx3 uint8, kps: (5, 2) px."""
    m = estimate_similarity(kps, ARCFACE_TEMPLATE * (size / FACE_SIZE))
    return cv2.warpAffine(
        rgb, m, (size, size), flags=cv2.INTER_LINEAR, borderMode=cv2.BORDER_CONSTANT, borderValue=0
    )


def kps_from_bbox(bbox: list[float], width: int, height: int) -> np.ndarray:
    """Fallback 5 points from a normalised [x, y, w, h] box (the template placed inside the box)."""
    x, y, w, h = bbox[0] * width, bbox[1] * height, bbox[2] * width, bbox[3] * height
    side = max(w, h)
    cx, cy = x + w / 2, y + h / 2
    return (ARCFACE_TEMPLATE / FACE_SIZE - 0.5) * side * 1.15 + np.array([cx, cy])


def _l2(x: np.ndarray) -> np.ndarray:
    return x / np.maximum(np.linalg.norm(x, axis=1, keepdims=True), 1e-12)


class _OrtFace:
    def __init__(self, session: Any):
        self.session = session
        self.input_name = session.get_inputs()[0].name
        self.output_name = session.get_outputs()[0].name
        shape = session.get_inputs()[0].shape
        self.dynamic_batch = not isinstance(shape[0], int)

    @property
    def providers(self) -> list[str]:
        return self.session.get_providers()

    def run(self, x: np.ndarray) -> np.ndarray:
        if self.dynamic_batch:
            try:
                return self.session.run([self.output_name], {self.input_name: x})[0]
            except Exception:  # noqa: BLE001  some exports have a static output batch dim
                self.dynamic_batch = False
        return np.concatenate(
            [
                self.session.run([self.output_name], {self.input_name: x[i : i + 1]})[0]
                for i in range(len(x))
            ]
        )

    def close(self) -> None:
        self.session = None


class IdentityEmbedder:
    """Loads AuraFace through the ModelManager; thread-safe (ORT sessions allow concurrent run)."""

    def __init__(self, manager: ModelManager, hw: HardwareInfo, force_cpu: bool = False):
        self.manager = manager
        self.hw = hw
        self.force_cpu = force_cpu or hw.device == "cpu"
        self._lock = threading.Lock()
        self.active_providers: list[str] = []

    def required_models(self) -> list[str]:
        return [MODEL_ID]

    def _handle(self) -> _OrtFace:
        def load(spec: ModelSpec, path: Path) -> _OrtFace:
            prov = ["CPUExecutionProvider"] if self.force_cpu else self.hw.providers
            return _OrtFace(_make_ort_session(path, prov))

        h = self.manager.acquire(MODEL_ID, load)
        self.active_providers = list(h.providers)
        return h

    def embed_aligned(self, crops: list[np.ndarray]) -> np.ndarray:
        """list of 112x112x3 RGB uint8 -> (N, 512) float32, L2-normalised."""
        if not crops:
            return np.zeros((0, IDENTITY_DIM), np.float32)
        x = (np.stack(crops).astype(np.float32) - 127.5) / 127.5
        x = np.ascontiguousarray(x.transpose(0, 3, 1, 2))
        with self._lock:
            h = self._handle()
        return _l2(np.asarray(h.run(x), dtype=np.float32))

    def embed_faces(self, rgb: np.ndarray, kps_list: list[Any]) -> np.ndarray:
        """Align + embed all faces of one image. kps_list: per face (5, 2) px landmarks."""
        crops = [align_face(rgb, np.asarray(k, dtype=np.float64)) for k in kps_list]
        return self.embed_aligned(crops)
