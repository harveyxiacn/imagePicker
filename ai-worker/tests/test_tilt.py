import math

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


def segments(
    segs: list[tuple[float, float, float, float]], w: int = 1024, h: int = 768
) -> np.ndarray:
    """Light gray image with dark antialiased segments (x0, y0, x1, y1) in pixels."""
    img = np.full((h, w), 170, np.uint8)
    for x0, y0, x1, y1 in segs:
        p0 = (int(round(x0 * 16)), int(round(y0 * 16)))
        p1 = (int(round(x1 * 16)), int(round(y1 * 16)))
        cv2.line(img, p0, p1, 70, 8, cv2.LINE_AA, shift=4)
    return cv2.GaussianBlur(img, (0, 0), 0.8)


def lean(
    x: float, y: float, deg: float, half: float, vertical: bool
) -> tuple[float, float, float, float]:
    """Segment through (x, y) px, `half` px each way, rotated clockwise by `deg` from the axis."""
    s, c = math.sin(math.radians(deg)), math.cos(math.radians(deg))
    dx, dy = (s * half, -c * half) if vertical else (c * half, s * half)
    return x - dx, y - dy, x + dx, y + dy


def test_one_sided_verticals_are_not_trusted():
    # a lone frame at the side leans as much under camera pitch (converging verticals) as under a
    # roll; frames on both sides leaning alike, or one through the centre, are a roll
    one = estimate_tilt(segments([lean(870, 384, 4.0, 370, True)]))
    assert one[1] < 0.05, one
    two = estimate_tilt(segments([lean(150, 384, 4.0, 370, True), lean(870, 384, 4.0, 370, True)]))
    assert abs(two[0] - 4.0) < 0.25 and two[1] > 0.3, two
    centre = estimate_tilt(segments([lean(512, 384, 4.0, 370, True)]))
    assert abs(centre[0] - 4.0) < 0.25 and centre[1] > 0.3, centre


def test_short_one_sided_stripes_are_not_a_horizon():
    # a striped shirt low in the frame: many short parallel lines, all on one side of the centre
    shirt = [lean(512, 540 + 20 * i, 6.0, 75, False) for i in range(9)]
    assert estimate_tilt(segments(shirt))[1] < 0.05
    # a full-width horizon at the same height counts wherever it sits
    tilt, conf = estimate_tilt(segments([lean(512, 620, 6.0, 600, False)]))
    assert abs(tilt - 6.0) < 0.25 and conf > 0.3, (tilt, conf)


def test_range_and_quality_output():
    q = analyze_quality(rotate(street(), 2.0))
    assert abs(q["tilt_deg"] - 2.0) < 0.25 and q["tilt_confidence"] > 0.4
    q = analyze_quality(synth_image(1, (1024, 768)))
    assert -TILT_MAX_DEG <= q["tilt_deg"] <= TILT_MAX_DEG
    assert 0.0 <= q["tilt_confidence"] <= 1.0


def _reference_line_profiles(g: np.ndarray) -> tuple[np.ndarray, np.ndarray]:
    """`line_profiles` as it was before the 2026-10-16 speed-up (full-image masks, full-length
    bincounts): the optimised version must give bit-identical profiles."""
    from imagepicker_ai.steps import tilt as t

    h, w = g.shape
    prof = np.zeros((2, t._K))
    if min(h, w) < 4 * t.MARGIN + 16:
        return prof[0], prof[1]
    gb = cv2.GaussianBlur(g.astype(np.float32), (0, 0), t.BLUR_SIGMA)
    gx = cv2.Sobel(gb, cv2.CV_32F, 1, 0, ksize=3).astype(np.float64)
    gy = cv2.Sobel(gb, cv2.CV_32F, 0, 1, ksize=3).astype(np.float64)
    mag = np.round(np.sqrt(gx * gx + gy * gy) * t.MAG_STEPS) / t.MAG_STEPS
    ax, ay = np.abs(gx), np.abs(gy)
    tt = math.tan(math.radians(t.TILT_MAX_DEG + t.GATE_DEG))
    strong = mag > t.EDGE_LO
    strong[: t.MARGIN] = strong[-t.MARGIN :] = False
    strong[:, : t.MARGIN] = strong[:, -t.MARGIN :] = False
    mp = np.pad(mag, 1)
    hfam = strong & (ax <= tt * ay) & (mag >= mp[:-2, 1:-1]) & (mag > mp[2:, 1:-1])
    vfam = strong & (ay <= tt * ax) & (mag >= mp[1:-1, :-2]) & (mag > mp[1:-1, 2:])
    line_min = t.LINE_MIN_FRAC * max(w, h)
    r0 = math.ceil(math.hypot(w, h) / 2) + 2
    n_rho = 2 * r0 + 2
    cx, cy = (w - 1) / 2.0, (h - 1) / 2.0
    for i, (fam, vertical) in enumerate(((hfam, False), (vfam, True))):
        ys, xs = np.nonzero(fam)
        gxs, gys = gx[ys, xs], gy[ys, xs]
        delta = np.degrees(np.arctan(gys / gxs if vertical else -gxs / gys))
        wgt = np.clip((mag[ys, xs] - t.EDGE_LO) / (t.EDGE_HI - t.EDGE_LO), 0.0, 1.0)
        acc = np.zeros(t._K * n_rho, np.float64)
        xc, yc = xs - cx, ys - cy
        k0 = np.ceil((delta - t.GATE_DEG + t.TILT_MAX_DEG) / t.BIN_DEG).astype(np.int64)
        for j in range(t._GATE_BINS + 1 if len(delta) else 0):
            k = k0 + j
            ok = (k >= 0) & (k < t._K)
            kk = np.where(ok, k, 0)
            dist = np.abs(t._THETA[kk] - delta)
            ok &= dist < t.GATE_DEG
            if not ok.any():
                continue
            kk, dd = kk[ok], dist[ok]
            wv = wgt[ok] * (1.0 - dd / t.GATE_DEG)
            c, s = t._COS[kk], t._SIN[kk]
            rho = (xc[ok] * c + yc[ok] * s if vertical else yc[ok] * c - xc[ok] * s) + r0
            r = np.floor(rho).astype(np.int64)
            f = rho - r
            base = kk * n_rho + r
            acc += np.bincount(base, wv * (1.0 - f), minlength=acc.size)
            acc += np.bincount(base + 1, wv * f, minlength=acc.size)
        prof[i] = t._line_profile(acc.reshape(t._K, n_rho), line_min)
    return prof[0], prof[1]


@pytest.mark.parametrize("deg", [0.0, 2.5, -7.0])
def test_fast_line_profiles_are_bit_identical(deg):
    from imagepicker_ai.steps.tilt import line_profiles

    imgs = [gray(rotate(street(), deg)), gray(jpeg_roundtrip(rotate(street(800, 600), deg), 70))]
    imgs += [gray(synth_image(s, (1024, 683))) for s in (11, 12)]
    imgs.append(np.full((120, 160), 128, np.uint8))  # no edges at all
    for g in imgs:
        ph, pv = line_profiles(g)
        rh, rv = _reference_line_profiles(g)
        np.testing.assert_array_equal(ph, rh)
        np.testing.assert_array_equal(pv, rv)
