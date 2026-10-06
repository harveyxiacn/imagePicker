"""Thin inference wrappers for the three mask networks. Each returns float32 probability maps.

BiRefNet           subject / person matting      ONNX (fixed input, 512 or 1024)  -> HxW alpha
SelfieMulticlass   hair / skin / clothes         LiteRT 256x256                   -> 256x256x6 softmax
SkySegmenter       sky                           ONNX U^2-Net 320x320             -> HxW probability
"""

from __future__ import annotations

import threading
from pathlib import Path
from typing import Any

import cv2
import numpy as np

from ..steps.embed import _make_ort_session

IMAGENET_MEAN = np.array([0.485, 0.456, 0.406], np.float32)
IMAGENET_STD = np.array([0.229, 0.224, 0.225], np.float32)

# MediaPipe selfie_multiclass_256x256 class indices
CLS_BACKGROUND, CLS_HAIR, CLS_BODY_SKIN, CLS_FACE_SKIN, CLS_CLOTHES, CLS_OTHERS = range(6)


def _squash(rgb: np.ndarray, w: int, h: int) -> np.ndarray:
    shrink = w < rgb.shape[1] or h < rgb.shape[0]
    return cv2.resize(rgb, (w, h), interpolation=cv2.INTER_AREA if shrink else cv2.INTER_CUBIC)


def _normalise(rgb: np.ndarray, w: int, h: int) -> np.ndarray:
    x = _squash(rgb, w, h).astype(np.float32) * (1.0 / 255.0)
    x = (x - IMAGENET_MEAN) / IMAGENET_STD
    return np.ascontiguousarray(x.transpose(2, 0, 1)[None])


def _sigmoid(x: np.ndarray) -> np.ndarray:
    return 1.0 / (1.0 + np.exp(-np.clip(x, -30.0, 30.0)))


class _Ort:
    def __init__(self, path: Path, providers: list[str], model_id: str):
        self.model_id = model_id
        self.session: Any = _make_ort_session(path, providers)
        inp = self.session.get_inputs()[0]
        self.input_name = inp.name
        self.half = "float16" in inp.type
        shape = inp.shape
        self.height = shape[2] if isinstance(shape[2], int) else 1024
        self.width = shape[3] if isinstance(shape[3], int) else 1024

    @property
    def providers(self) -> list[str]:
        return self.session.get_providers()

    def _run(self, x: np.ndarray) -> np.ndarray:
        if self.half:
            x = x.astype(np.float16)
        return self.session.run(None, {self.input_name: x})[0]

    def close(self) -> None:
        self.session = None


class BiRefNet(_Ort):
    """Foreground alpha in [0, 1] at the size of the input image (BiRefNet outputs logits)."""

    def predict(self, rgb: np.ndarray) -> np.ndarray:
        h, w = rgb.shape[:2]
        y = self._run(_normalise(rgb, self.width, self.height))
        a = _sigmoid(np.asarray(y, dtype=np.float32).reshape(y.shape[-2], y.shape[-1]))
        return cv2.resize(a, (w, h), interpolation=cv2.INTER_LINEAR)


class SkySegmenter(_Ort):
    """U^2-Net sky probability in [0, 1] at the size of the input image."""

    def predict(self, rgb: np.ndarray) -> np.ndarray:
        h, w = rgb.shape[:2]
        outs = self._run(_normalise(rgb, self.width, self.height))
        m = np.asarray(outs, dtype=np.float32).reshape(outs.shape[-2], outs.shape[-1])
        # the fused output is already a sigmoid probability; the reference demo's min-max
        # normalisation would blow noise up to "sky" on frames without any sky, so it is skipped
        m = np.clip(m, 0.0, 1.0)
        return cv2.resize(m, (w, h), interpolation=cv2.INTER_LINEAR)


class SelfieMulticlass:
    """MediaPipe selfie multiclass segmenter via LiteRT (interpreter calls are serialised)."""

    SIZE = 256

    def __init__(self, path: Path, model_id: str = "mediapipe-selfie-multiclass"):
        from ai_edge_litert.interpreter import Interpreter

        self.model_id = model_id
        self._lock = threading.Lock()
        self._interp = Interpreter(model_path=str(path))
        self._interp.allocate_tensors()
        self._in = self._interp.get_input_details()[0]["index"]
        self._out = self._interp.get_output_details()[0]["index"]
        self.providers = ["LiteRT:cpu"]

    def predict(self, rgb: np.ndarray) -> np.ndarray:
        """Softmax class probabilities, 256x256x6 float32 (squashed input, caller resizes)."""
        s = self.SIZE
        x = _squash(rgb, s, s).astype(np.float32) * (1.0 / 255.0)
        with self._lock:
            self._interp.set_tensor(self._in, x[None])
            self._interp.invoke()
            logits = np.array(self._interp.get_tensor(self._out)[0], dtype=np.float32)
        logits -= logits.max(axis=-1, keepdims=True)
        e = np.exp(logits)
        return e / e.sum(axis=-1, keepdims=True)

    def close(self) -> None:
        self._interp = None
