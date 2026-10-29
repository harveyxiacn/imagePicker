"""Optional native acceleration of the `quality` step: `crates/ip-lite-ffi` (a C ABI around the
Rust port of this same step) loaded with `ctypes`.

The worker still has the pure-Python implementation in `quality.py`; this module only reports
whether the library is there and how to call it. `ctypes.CDLL` releases the GIL for the call, so
the decode threads run the analysis in parallel — the point of the whole exercise.

The library is looked for, in order:
  * `$IMAGEPICKER_LITE_LIB` (the desktop shell sets it from its resources; `0` / `off` disables
    the native path, which the benchmarks use for an A/B run),
  * `imagepicker_ai/steps/_native/<name>` (when one is shipped inside the worker source),
  * `target/{release,debug}/<name>` of the checkout the module lives in (development, tests).

Without a library — or on an ABI mismatch — `quality()` returns None and `quality.py` keeps using
its Python path, so behaviour never depends on the file being present.
"""

from __future__ import annotations

import ctypes
import logging
import os
import sys
from pathlib import Path

import numpy as np

log = logging.getLogger(__name__)

#: Must match `ip_lite_ffi::ABI_VERSION` (2: `sharpness` also looks at the sharpest grid cell).
ABI_VERSION = 2

#: Order of `QualityOut`'s doubles after `status` (crates/ip-lite-ffi/src/lib.rs).
FIELD_NAMES = (
    "sharpness",
    "sharpness_center",
    "laplacian_var",
    "tenengrad",
    "exposure",
    "mean_luminance",
    "clipped_highlights",
    "crushed_shadows",
    "noise",
    "noise_sigma",
    "tilt_deg",
    "tilt_confidence",
)


class _QualityOut(ctypes.Structure):
    _fields_ = [("status", ctypes.c_int), *((n, ctypes.c_double) for n in FIELD_NAMES)]


def _library_name() -> str:
    if sys.platform == "win32":
        return "ip_lite_ffi.dll"
    if sys.platform == "darwin":
        return "libip_lite_ffi.dylib"
    return "libip_lite_ffi.so"


def _candidates() -> list[Path]:
    name = _library_name()
    out: list[Path] = []
    env = os.environ.get("IMAGEPICKER_LITE_LIB")
    if env:
        out.append(Path(env))
    here = Path(__file__).resolve()
    out.append(here.parent / "_native" / name)
    for base in here.parents:  # the checkout's build output (development, benchmark, tests)
        for profile in ("release", "debug"):
            out.append(base / "target" / profile / name)
        if (base / "Cargo.toml").is_file():
            break
    return out


_state: dict[str, object] = {"lib": None, "tried": False, "why": "not loaded yet"}


def _load() -> ctypes.CDLL | None:
    if _state["tried"]:
        return _state["lib"]  # type: ignore[return-value]
    _state["tried"] = True
    if os.environ.get("IMAGEPICKER_LITE_LIB", "").strip().lower() in ("0", "off", "none", "no"):
        _state["why"] = "disabled by IMAGEPICKER_LITE_LIB"
        return None
    for p in _candidates():
        if not p.is_file():
            continue
        try:
            lib = ctypes.CDLL(str(p))
            lib.imagepicker_lite_abi_version.restype = ctypes.c_uint32
            version = int(lib.imagepicker_lite_abi_version())
            if version != ABI_VERSION:
                _state["why"] = f"{p}: ABI {version}, expected {ABI_VERSION}"
                continue
            lib.imagepicker_lite_quality.argtypes = [
                ctypes.POINTER(ctypes.c_ubyte),
                ctypes.c_uint32,
                ctypes.c_uint32,
                ctypes.POINTER(_QualityOut),
            ]
            lib.imagepicker_lite_quality.restype = ctypes.c_int
        except (OSError, AttributeError) as e:  # not a library, wrong build, missing symbol
            _state["why"] = f"{p}: {e}"
            continue
        _state["lib"], _state["why"] = lib, f"loaded {p}"
        log.info("quality: native ip-lite loaded from %s", p)
        return lib
    log.info("quality: native ip-lite unavailable (%s); using the Python step", _state["why"])
    return None


def available() -> bool:
    """Whether the native library loaded (loads it on first use)."""
    return _load() is not None


def status() -> str:
    """Where the library came from, or why it is not used (`/api/worker` diagnostics, tests)."""
    _load()
    return str(_state["why"])


def quality(rgb: np.ndarray) -> dict[str, float] | None:
    """The step's numeric fields for an HxWx3 uint8 RGB image, or None when unavailable.

    The values are already rounded exactly like `quality.analyze_quality` (`round(x, 5)`, tilt
    to 2 and its confidence to 3 decimals); face sharpness and saliency stay in Python.
    """
    lib = _load()
    if lib is None:
        return None
    arr = np.ascontiguousarray(rgb)
    if arr.dtype != np.uint8 or arr.ndim != 3 or arr.shape[2] != 3:
        return None
    h, w = int(arr.shape[0]), int(arr.shape[1])
    out = _QualityOut()
    rc = lib.imagepicker_lite_quality(
        arr.ctypes.data_as(ctypes.POINTER(ctypes.c_ubyte)), w, h, ctypes.byref(out)
    )
    if rc != 0:
        log.warning("quality: native analysis failed (rc=%d); using the Python step", rc)
        return None
    return {n: float(getattr(out, n)) for n in FIELD_NAMES}


def _reset_for_tests() -> None:
    _state["lib"], _state["tried"], _state["why"] = None, False, "not loaded yet"
