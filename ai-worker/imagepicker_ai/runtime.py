"""ONNX session handling for the generative models (doc 03 section 8.2).

`GenRuntime` wraps `ModelManager.acquire` for ONNX models and implements the OOM policy:
  1. the operation is retried with half the tile size (down to `MIN_TILE`),
  2. then the model is reloaded on the CPU provider and the operation restarts,
  3. and only then `OutOfMemory` (-32012) is raised.
The CPU fallback is per request: the next request tries the GPU again (a session loaded on the
other device is reloaded on demand).
"""

from __future__ import annotations

import logging
import threading
from collections.abc import Callable
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from typing import Any, TypeVar

import numpy as np

from .errors import OutOfMemory
from .hw import HardwareInfo
from .models.manager import ModelManager
from .steps.embed import _is_oom, _make_ort_session

log = logging.getLogger(__name__)

MIN_TILE = 128
T = TypeVar("T")


class OrtHandle:
    """An ORT session of one model + the device it lives on."""

    def __init__(self, path: Path, providers: list[str], model_id: str):
        self.model_id = model_id
        self.cpu = providers == ["CPUExecutionProvider"]
        self.session: Any = _make_ort_session(path, providers)
        self.inputs = {i.name: i for i in self.session.get_inputs()}
        self.input_name = self.session.get_inputs()[0].name
        self.half = "float16" in self.session.get_inputs()[0].type
        self._lock = threading.Lock()

    @property
    def providers(self) -> list[str]:
        return self.session.get_providers()

    def run(self, feeds: dict[str, np.ndarray]) -> np.ndarray:
        if self.half:
            feeds = {
                k: (v.astype(np.float16) if v.dtype == np.float32 else v) for k, v in feeds.items()
            }
        with self._lock:
            out = self.session.run(None, feeds)[0]
        return np.asarray(out, dtype=np.float32) if out.dtype != np.float32 else out

    def close(self) -> None:
        self.session = None


class GenRuntime:
    def __init__(self, manager: ModelManager, hw: HardwareInfo):
        self.manager = manager
        self.hw = hw
        self.cpu_only = hw.device == "cpu"
        # one worker thread for inpaint / enhance: serialises the big models (VRAM) and keeps
        # the event loop free
        self.pool = ThreadPoolExecutor(1, thread_name_prefix="gen")

    def shutdown(self) -> None:
        self.pool.shutdown(wait=False, cancel_futures=True)

    def providers(self, force_cpu: bool = False) -> list[str]:
        return ["CPUExecutionProvider"] if (force_cpu or self.cpu_only) else self.hw.providers

    def session(self, model_id: str, force_cpu: bool = False) -> OrtHandle:
        want_cpu = self.providers(force_cpu) == ["CPUExecutionProvider"]
        h: OrtHandle = self.manager.acquire(
            model_id,
            lambda spec, path: OrtHandle(path, self.providers(force_cpu), spec.id),
        )
        if h.cpu != want_cpu:
            # loaded on the other device by an earlier request: reload on the wanted one
            self.manager.unload(model_id)
            h = self.manager.acquire(
                model_id, lambda spec, path: OrtHandle(path, self.providers(force_cpu), spec.id)
            )
        return h

    def run_with_backoff(
        self,
        model_id: str,
        work: Callable[[OrtHandle, int], T],
        tile: int,
        cpu_tile: int | None = None,
    ) -> T:
        """`work(handle, tile)` with the OOM policy above. `tile` is the GPU tile edge in pixels."""
        force_cpu = False
        cur = tile
        while True:
            h = self.session(model_id, force_cpu)
            try:
                return work(h, cur)
            except Exception as e:  # noqa: BLE001
                if not _is_oom(e):
                    raise
                if cur > MIN_TILE:
                    cur = max(MIN_TILE, cur // 2)
                    log.warning("OOM in %s; tile -> %d", model_id, cur)
                    self.manager.unload(model_id)  # drops the arena grown by the failed run
                    continue
                if not force_cpu and not self.cpu_only:
                    log.warning("OOM at the minimum tile in %s; falling back to CPU", model_id)
                    self.manager.unload(model_id)
                    force_cpu = True
                    cur = cpu_tile or min(tile, 256)
                    continue
                raise OutOfMemory(str(e)) from e
