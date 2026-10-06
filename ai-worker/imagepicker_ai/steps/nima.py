"""NIMA aesthetic / technical image quality (MobileNet, LiteRT fp16, ~6 MB each, CPU ~3-5 ms).

Weights: idealo/image-quality-assessment (Apache-2.0) via litert-community/NIMA-LiteRT.
Input 224x224 RGB squashed (the same tensor the SigLIP2 path uses, scaled to [-1, 1]); output is a
softmax over the scores 1..10 and the model score is its mean.

  aesthetic  trained on AVA            typical photos land at 4.5-5.5, excellent ones > 6
  technical  trained on TID2013        ~3 for heavy degradation, 6+ for clean images

Calibration to 0-1 (higher = better) -- logistic, centred on what a typical photo scores:
  aesthetic01 = sigmoid((score - 5.0) / 0.65)       5.0 -> 0.50,  6.0 -> 0.82,  4.0 -> 0.18
  technical01 = sigmoid((score - 5.0) / 0.90)       5.0 -> 0.50,  6.0 -> 0.75,  3.5 -> 0.16
"""

from __future__ import annotations

import math
import threading
from pathlib import Path

import numpy as np

AESTHETIC_ID = "nima-aesthetic"
TECHNICAL_ID = "nima-technical"

AESTHETIC_CENTER, AESTHETIC_SCALE = 5.0, 0.65
TECHNICAL_CENTER, TECHNICAL_SCALE = 5.0, 0.90

_BINS = np.arange(1, 11, dtype=np.float32)


def _sigmoid(x: float) -> float:
    return 1.0 / (1.0 + math.exp(-x))


def calibrate_aesthetic(score: float) -> float:
    return _sigmoid((score - AESTHETIC_CENTER) / AESTHETIC_SCALE)


def calibrate_technical(score: float) -> float:
    return _sigmoid((score - TECHNICAL_CENTER) / TECHNICAL_SCALE)


def litert_available() -> bool:
    try:
        from ai_edge_litert.interpreter import Interpreter  # noqa: F401

        return True
    except Exception:  # noqa: BLE001
        return False


class NimaModel:
    """One tflite model; interpreters are created lazily per thread (they are not thread-safe)."""

    def __init__(self, path: str | Path):
        self.path = str(path)
        self._tls = threading.local()

    def _interp(self):
        it = getattr(self._tls, "it", None)
        if it is None:
            from ai_edge_litert.interpreter import Interpreter

            it = Interpreter(model_path=self.path, num_threads=1)
            it.allocate_tensors()
            self._tls.it = it
            self._tls.inp = it.get_input_details()[0]["index"]
            self._tls.out = it.get_output_details()[0]["index"]
        return it

    def score(self, prepared: np.ndarray) -> float:
        """224x224x3 uint8 RGB -> raw NIMA mean score in [1, 10]."""
        it = self._interp()
        x = (prepared.astype(np.float32) * (1 / 127.5) - 1.0)[None]
        it.set_tensor(self._tls.inp, x)
        it.invoke()
        dist = it.get_tensor(self._tls.out)[0].astype(np.float32)
        s = float(dist.sum())
        if s <= 0:
            return 5.0
        return float((_BINS * dist).sum() / s)

    def close(self) -> None:
        self._tls = threading.local()
