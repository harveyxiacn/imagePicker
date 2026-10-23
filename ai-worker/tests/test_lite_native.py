"""The optional native `quality` step (`steps/lite_native.py` + `crates/ip-lite-ffi`).

Skipped when no library was built: `cargo build -p ip-lite-ffi` (or `cargo test --workspace`)
puts `ip_lite_ffi.dll` / `libip_lite_ffi.{so,dylib}` in `target/{release,debug}`, where the loader
finds it from the checkout.
"""

from __future__ import annotations

import numpy as np
import pytest

from imagepicker_ai.steps import lite_native
from imagepicker_ai.steps import quality as q


def scene(w: int = 320, h: int = 240, tilt: float = 0.0, seed: int = 0) -> np.ndarray:
    """Deterministic RGB image: gradient, blocks and a horizon rotated by `tilt` degrees."""
    rng = np.random.default_rng(seed)
    y, x = np.mgrid[0:h, 0:w]
    img = np.stack([x * 0.4, y * 0.5, (x + y) * 0.2], axis=-1)
    img[(x // 40 + y // 40) % 2 == 0] += rng.uniform(-30, 30, 3)
    if tilt:
        a = np.deg2rad(tilt)
        d = (y - h / 2) * np.cos(a) - (x - w / 2) * np.sin(a)
        img[d > 30] = [230, 235, 240]
        img[d < -40] = [40, 70, 50]
    else:
        img[: h // 3] = [230, 235, 200]
        img[-h // 4 :] = [60, 110, 70]
    return np.clip(img, 0, 255).astype(np.uint8)


def without_native(monkeypatch):
    monkeypatch.setattr(lite_native, "quality", lambda _rgb: None)


def close(got: float, want: float, abs_tol: float, rel_tol: float = 0.004) -> bool:
    return abs(got - want) <= max(abs_tol, rel_tol * abs(want))


requires_native = pytest.mark.skipif(
    not lite_native.available(), reason=f"native ip-lite not built: {lite_native.status()}"
)


@requires_native
@pytest.mark.parametrize("tilt,seed", [(0.0, 0), (3.0, 1), (-2.0, 2)])
def test_native_matches_the_python_step(monkeypatch, tilt: float, seed: int) -> None:
    rgb = scene(tilt=tilt, seed=seed)
    native = q.analyze_quality(rgb, [(0.3, 0.2, 0.2, 0.2)])
    without_native(monkeypatch)
    python = q.analyze_quality(rgb, [(0.3, 0.2, 0.2, 0.2)])
    # the same tolerances as crates/ip-lite/tests/parity.rs
    assert close(native["sharpness"], python["sharpness"], 2e-3)
    assert close(native["sharpness_center"], python["sharpness_center"], 2e-3)
    assert close(native["laplacian_var"], python["laplacian_var"], 0.02)
    assert close(native["tenengrad"], python["tenengrad"], 0.5)
    for key in ("exposure", "mean_luminance", "clipped_highlights", "crushed_shadows"):
        assert close(native[key], python[key], 1e-4)
    assert close(native["noise"], python["noise"], 5e-3)
    assert close(native["noise_sigma"], python["noise_sigma"], 0.05)
    assert close(native["tilt_confidence"], python["tilt_confidence"], 0.01)
    if python["tilt_confidence"] >= 0.05:
        assert close(native["tilt_deg"], python["tilt_deg"], 0.05, 1e-9)
    # the fields the Python step adds on top are the same either way
    assert native["sharpness_face"] == python["sharpness_face"]
    assert native["saliency"] == python["saliency"]
    assert native["sharpness"] == native["sharpness_center"] or native["sharpness_face"] is not None


@requires_native
def test_native_rejects_a_non_rgb_buffer(monkeypatch) -> None:
    assert lite_native.quality(np.zeros((8, 8), np.uint8)) is None
    assert lite_native.quality(np.zeros((8, 8, 3), np.float32)) is None


def test_fallback_without_the_library(monkeypatch) -> None:
    without_native(monkeypatch)
    res = q.analyze_quality(scene())
    for key in ("sharpness", "sharpness_center", "laplacian_var", "tenengrad", "tilt_deg"):
        assert key in res
    assert "saliency" in res


def test_off_switch_disables_the_native_path(monkeypatch) -> None:
    monkeypatch.setenv("IMAGEPICKER_LITE_LIB", "0")
    lite_native._reset_for_tests()
    try:
        assert not lite_native.available()
        assert "disabled" in lite_native.status()
        assert lite_native.quality(scene()) is None
    finally:
        monkeypatch.delenv("IMAGEPICKER_LITE_LIB")
        lite_native._reset_for_tests()
