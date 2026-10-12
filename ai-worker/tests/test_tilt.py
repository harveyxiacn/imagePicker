import cv2
import numpy as np
import pytest

from imagepicker_ai.steps.quality import analyze_quality
from imagepicker_ai.steps.tilt import TILT_MAX_DEG, estimate_tilt

from .conftest import jpeg_roundtrip, synth_image


def street(w: int = 1024, h: int = 768) -> np.ndarray:
    """Level RGB scene: sky over sea, a few poles and a building with window rows."""
    img = np.zeros((h, w, 3), np.float32)
    img[: int(0.55 * h)] = (150, 185, 225)
    img[int(0.55 * h) :] = (40, 80, 120)
    for x in (0.12, 0.3, 0.83):
        px = int(x * w)
        cv2.rectangle(img, (px, int(0.2 * h)), (px + 8, h - 1), (35, 30, 30), -1)
    x0, x1 = int(0.5 * w), int(0.75 * w)
    cv2.rectangle(img, (x0, int(0.3 * h)), (x1, int(0.55 * h)), (190, 170, 150), -1)
    for i in range(1, 5):
        y = int(0.3 * h) + i * h // 25
        cv2.line(img, (x0, y), (x1, y), (90, 80, 70), 2)
    return cv2.GaussianBlur(img, (0, 0), 0.8).clip(0, 255).astype(np.uint8)


def rotate(rgb: np.ndarray, deg: float) -> np.ndarray:
    """Content rotated clockwise by `deg` (the tilt convention), borders reflected."""
    h, w = rgb.shape[:2]
    m = cv2.getRotationMatrix2D(((w - 1) / 2, (h - 1) / 2), -deg, 1.0)
    return cv2.warpAffine(rgb, m, (w, h), flags=cv2.INTER_CUBIC, borderMode=cv2.BORDER_REFLECT_101)


def gray(rgb: np.ndarray) -> np.ndarray:
    return cv2.cvtColor(rgb, cv2.COLOR_RGB2GRAY)


@pytest.mark.parametrize("deg", [0.0, 1.5, -3.0, 6.0, -9.0])
def test_known_rotation_is_recovered(deg):
    tilt, conf = estimate_tilt(gray(rotate(street(), deg)))
    assert abs(tilt - deg) < 0.25, (tilt, conf)
    assert conf > 0.4, (tilt, conf)


def test_sign_convention_horizon_falls_to_the_right():
    # a clockwise rotation moves the right end of the horizon down
    rgb = rotate(street(), 4.0)
    g = gray(rgb).astype(np.float32)
    w = g.shape[1]

    def horizon_row(x: int) -> int:
        return int(np.argmax(np.abs(np.diff(g[:, x]))))

    assert horizon_row(int(0.95 * w)) > horizon_row(int(0.05 * w))
    assert estimate_tilt(gray(rgb))[0] > 0


def test_survives_jpeg_and_noise():
    rgb = rotate(street(), -2.5)
    rgb = jpeg_roundtrip(rgb, 80)
    noisy = np.clip(rgb + np.random.default_rng(0).normal(0, 6, rgb.shape), 0, 255).astype(np.uint8)
    tilt, conf = estimate_tilt(gray(noisy))
    assert abs(tilt + 2.5) < 0.3 and conf > 0.3, (tilt, conf)


def test_no_straight_structure_gives_no_confidence():
    assert estimate_tilt(np.full((480, 640), 128, np.uint8)) == (0.0, 0.0)
    assert estimate_tilt(np.zeros((20, 20), np.uint8)) == (0.0, 0.0)
    rng = np.random.default_rng(3)
    blobs = cv2.GaussianBlur(rng.uniform(0, 255, (768, 1024)).astype(np.float32), (0, 0), 6)
    blobs = np.clip((blobs - 128) * 12 + 128, 0, 255).astype(np.uint8)  # steep curved outlines
    assert estimate_tilt(blobs)[1] < 0.15


def test_range_and_quality_output():
    q = analyze_quality(rotate(street(), 2.0))
    assert abs(q["tilt_deg"] - 2.0) < 0.25 and q["tilt_confidence"] > 0.4
    q = analyze_quality(synth_image(1, (1024, 768)))
    assert -TILT_MAX_DEG <= q["tilt_deg"] <= TILT_MAX_DEG
    assert 0.0 <= q["tilt_confidence"] <= 1.0
