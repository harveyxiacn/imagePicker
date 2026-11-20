"""The GPU out-of-memory fallback to CPU, as a cool-down instead of a permanent switch.

After an out-of-memory error the mask / beauty / Best Take / hair pool (`MaskGenerator`) and the
analysis embedder run on the CPU provider. A transient OOM (another program holding VRAM for a
minute) must not pin them there until the app restarts: once the cool-down is over the next request
tries the GPU again, and a repeated OOM re-arms a longer cool-down so a GPU that is simply too small
does not pay an OOM every ten minutes. A machine that is CPU-only from the start never leaves it.
"""

from __future__ import annotations

import logging
import threading
import time
from collections.abc import Callable

log = logging.getLogger(__name__)

OOM_COOLDOWN_S = 600.0  # first OOM: stay on the CPU this long
OOM_COOLDOWN_MAX_S = 3600.0  # repeated OOMs double the cool-down up to this


class CpuFallback:
    """`active` is the old `force_cpu` flag; readers keep asking it, so nothing caches the value."""

    def __init__(
        self,
        cpu_only: bool,
        *,
        on_retry_gpu: Callable[[], object] | None = None,
        clock: Callable[[], float] = time.monotonic,
        cooldown_s: float = OOM_COOLDOWN_S,
        max_cooldown_s: float = OOM_COOLDOWN_MAX_S,
    ):
        self.cpu_only = cpu_only
        self._on_retry_gpu = on_retry_gpu
        self._clock = clock
        self._cooldown_s = cooldown_s
        self._max_cooldown_s = max_cooldown_s
        self._lock = threading.Lock()
        self._until: float | None = None  # cool-down end; None = the GPU is allowed
        self._last_end = 0.0  # when the previous cool-down ended
        self._strikes = 0

    @property
    def active(self) -> bool:
        """True while requests must use the CPU provider."""
        if self.cpu_only:
            return True
        with self._lock:
            if self._until is None:
                return False
            now = self._clock()
            if now < self._until:
                return True
            self._last_end, self._until = self._until, None
        log.info("OOM cool-down over; the next request tries the GPU again")
        if self._on_retry_gpu is not None:
            try:
                self._on_retry_gpu()  # drop the handles that were loaded on the CPU provider
            except Exception:  # noqa: BLE001
                log.exception("releasing the CPU-loaded models failed")
        return False

    def trip(self) -> float:
        """An OOM happened: use the CPU for the next cool-down. Returns its length in seconds."""
        if self.cpu_only:
            return 0.0
        with self._lock:
            now = self._clock()
            if self._strikes and now - self._last_end > self._max_cooldown_s:
                self._strikes = 0  # long quiet since the last episode: treat this one as new
            self._strikes += 1
            length = min(self._cooldown_s * 2 ** (self._strikes - 1), self._max_cooldown_s)
            self._until = now + length
        log.warning("GPU out of memory: using the CPU for %.0f s", length)
        return length
