"""ModelManager: install state, VRAM budget, LRU unloading (doc 03 §8.2).

Budget = VRAM_total * 0.85 - render-engine reserve (1.5 GB). On CPU-only machines the
budget is half of system RAM (RAM numbers are used instead of VRAM numbers).

Loading rules (`acquire`):
  * a model already loaded is returned and its LRU timestamp refreshed;
  * otherwise models of the same `exclusive_group` are evicted, then least-recently-used
    models (non-resident first, then resident) are evicted until the new one fits;
  * if it still does not fit -> OutOfMemory (caller may fall back to CPU / smaller model);
  * `sweep_idle(max_idle_s)` unloads non-resident models that have been idle (frees VRAM for games/editors).
"""

from __future__ import annotations

import gc
import logging
import threading
import time
from collections.abc import Callable, Iterator
from contextlib import contextmanager
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from ..errors import ModelUnavailable, OutOfMemory
from ..hw import HardwareInfo
from .download import Downloader, ProgressCb
from .registry import ModelSpec, Registry

log = logging.getLogger(__name__)

RENDER_RESERVE_MB = 1536
VRAM_FRACTION = 0.85


@dataclass
class _Loaded:
    spec: ModelSpec
    handle: Any
    cost_mb: int
    loaded_at: float = field(default_factory=time.monotonic)
    last_used: float = field(default_factory=time.monotonic)
    in_use: int = 0


def compute_budget_mb(hw: HardwareInfo | None, override: int | None = None) -> int:
    if override is not None:
        return override
    if hw is None:
        return 4096
    gpu = hw.primary_gpu
    if hw.device != "cpu" and gpu and gpu.vram_mb:
        if gpu.unified_memory:  # Apple: share with the OS and the render engine
            return int(gpu.vram_mb * 0.5)
        return max(512, int(gpu.vram_mb * VRAM_FRACTION) - RENDER_RESERVE_MB)
    return int(hw.ram_mb * 0.5)


class ModelManager:
    def __init__(
        self,
        models_dir: Path,
        registry: Registry,
        hw: HardwareInfo | None = None,
        budget_mb: int | None = None,
        downloader: Downloader | None = None,
    ):
        self.models_dir = Path(models_dir)
        self.registry = registry
        self.hw = hw
        self.budget_mb = compute_budget_mb(hw, budget_mb)
        self.downloader = downloader or Downloader(self.models_dir)
        self._loaded: dict[str, _Loaded] = {}
        self._lock = threading.RLock()
        self._load_locks: dict[str, threading.Lock] = {}

    # ------------------------------------------------------------------ install state
    def is_installed(self, model_id: str) -> bool:
        return self.downloader.is_installed(self.registry.get(model_id))

    def path(self, model_id: str) -> Path:
        """Primary file path of an installed model; ModelUnavailable if missing."""
        spec = self.registry.get(model_id)
        if not self.downloader.is_installed(spec):
            raise ModelUnavailable(f"model {model_id!r} is not installed", [model_id])
        return self.downloader.file_path(spec)

    def dir(self, model_id: str) -> Path:
        return self.downloader.install_dir(self.registry.get(model_id))

    def ensure(
        self,
        model_id: str,
        progress: ProgressCb | None = None,
        cancel: threading.Event | None = None,
    ) -> Path:
        """Blocking: download if needed. Call from a worker thread."""
        return self.downloader.ensure(self.registry.get(model_id), progress, cancel)

    def delete(self, model_id: str) -> None:
        self.unload(model_id)
        self.downloader.remove(self.registry.get(model_id))

    # ------------------------------------------------------------------ loading
    def _cost_mb(self, spec: ModelSpec) -> int:
        on_gpu = self.hw is not None and self.hw.device != "cpu"
        return spec.vram_mb if on_gpu else spec.ram_mb

    def used_mb(self) -> int:
        with self._lock:
            return sum(m.cost_mb for m in self._loaded.values())

    def acquire(
        self,
        model_id: str,
        loader: Callable[[ModelSpec, Path], Any],
        cost_mb: int | None = None,
    ) -> Any:
        """Return a loaded handle for `model_id`, loading (and evicting) as necessary.

        `cost_mb` overrides the registry cost for this load (e.g. a diffusion pipeline loaded with
        CPU offload needs far less VRAM than its fully resident registry figure).
        """
        spec = self.registry.get(model_id)
        with self._lock:
            m = self._loaded.get(model_id)
            if m:
                m.last_used = time.monotonic()
                return m.handle
            load_lock = self._load_locks.setdefault(model_id, threading.Lock())
        with load_lock:  # one concurrent load per model, others wait then reuse
            with self._lock:
                m = self._loaded.get(model_id)
                if m:
                    m.last_used = time.monotonic()
                    return m.handle
                self._make_room(spec, cost_mb)
            path = self.path(model_id)
            t0 = time.monotonic()
            handle = loader(spec, path)
            log.info("loaded %s in %.2fs", model_id, time.monotonic() - t0)
            with self._lock:
                self._loaded[model_id] = _Loaded(
                    spec, handle, cost_mb if cost_mb is not None else self._cost_mb(spec)
                )
            return handle

    @contextmanager
    def lease(
        self,
        model_id: str,
        loader: Callable[[ModelSpec, Path], Any],
        cost_mb: int | None = None,
    ) -> Iterator[Any]:
        """`acquire` + mark the model in use for the duration (never evicted / idle-unloaded)."""
        handle = self.acquire(model_id, loader, cost_mb)
        with self._lock:
            m = self._loaded.get(model_id)
            if m:
                m.in_use += 1
        try:
            yield handle
        finally:
            with self._lock:
                m = self._loaded.get(model_id)
                if m:
                    m.in_use = max(0, m.in_use - 1)
                    m.last_used = time.monotonic()

    def _make_room(self, spec: ModelSpec, cost_mb: int | None = None) -> None:
        need = cost_mb if cost_mb is not None else self._cost_mb(spec)
        if need > self.budget_mb:
            raise OutOfMemory(
                f"model {spec.id} needs {need} MB but the budget is {self.budget_mb} MB"
            )
        if spec.exclusive_group:
            for mid, m in list(self._loaded.items()):
                if m.spec.exclusive_group == spec.exclusive_group:
                    if m.in_use:
                        raise OutOfMemory(
                            f"{mid} is busy; cannot load {spec.id} (exclusive group "
                            f"{spec.exclusive_group})"
                        )
                    self._unload_locked(mid)
        while self.used_mb() + need > self.budget_mb:
            victims = [(mid, m) for mid, m in self._loaded.items() if m.in_use == 0]
            if not victims:
                raise OutOfMemory(f"cannot fit {spec.id} ({need} MB): all loaded models are in use")
            # non-resident first, then least recently used
            mid, _ = min(victims, key=lambda kv: (kv[1].spec.resident, kv[1].last_used))
            log.info("evicting %s (LRU) to fit %s", mid, spec.id)
            self._unload_locked(mid)

    def touch(self, model_id: str) -> None:
        with self._lock:
            if model_id in self._loaded:
                self._loaded[model_id].last_used = time.monotonic()

    def unload(self, model_id: str | None = None) -> list[str]:
        """Unload one model (or all when None). Returns the ids actually unloaded."""
        with self._lock:
            ids = [model_id] if model_id else list(self._loaded)
            return [i for i in ids if self._unload_locked(i)]

    def _unload_locked(self, model_id: str) -> bool:
        m = self._loaded.pop(model_id, None)
        if m is None:
            return False
        close = getattr(m.handle, "close", None)
        if callable(close):
            try:
                close()
            except Exception:  # noqa: BLE001
                log.exception("closing %s failed", model_id)
        del m
        gc.collect()
        _release_torch_cache()
        return True

    def sweep_idle(self, max_idle_s: float, include_resident: bool = False) -> list[str]:
        now = time.monotonic()
        with self._lock:
            idle = [
                mid
                for mid, m in self._loaded.items()
                if m.in_use == 0
                and now - m.last_used > max_idle_s
                and (include_resident or not m.spec.resident)
            ]
            return [i for i in idle if self._unload_locked(i)]

    # ------------------------------------------------------------------ reporting
    def loaded(self) -> list[dict[str, Any]]:
        now = time.monotonic()
        with self._lock:
            return [
                {
                    "id": mid,
                    "cost_mb": m.cost_mb,
                    "idle_s": round(now - m.last_used, 1),
                    "resident": m.spec.resident,
                    "exclusive_group": m.spec.exclusive_group,
                    "in_use": m.in_use > 0,
                    "providers": list(getattr(m.handle, "providers", None) or []),
                }
                for mid, m in self._loaded.items()
            ]

    def list_models(self) -> list[dict[str, Any]]:
        tier = self.hw.tier if self.hw else None
        out = []
        for s in self.registry:
            d = s.to_public()
            d["installed"] = self.downloader.is_installed(s)
            d["loaded"] = s.id in self._loaded
            d["recommended"] = bool(tier and tier in s.tiers)
            out.append(d)
        return out


def _release_torch_cache() -> None:
    import sys

    t = sys.modules.get("torch")
    if t is not None:
        try:
            if t.cuda.is_available():
                t.cuda.empty_cache()
        except Exception:  # noqa: BLE001
            pass
