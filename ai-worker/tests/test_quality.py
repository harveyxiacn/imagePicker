import cv2
import numpy as np

from imagepicker_ai.steps.quality import analyze_quality, face_sharpness

from .conftest import synth_image


def _q(img, boxes=None):
    return analyze_quality(img, boxes)


def test_ranges():
    q = _q(synth_image(1, (1024, 768)))
    for k in (
        "sharpness",
        "sharpness_center",
        "exposure",
        "noise",
        "mean_luminance",
        "clipped_highlights",
        "crushed_shadows",
    ):
        assert 0.0 <= q[k] <= 1.0, k
    assert q["sharpness_face"] is None
    assert q["sharpness"] >= q["sharpness_center"]  # without faces: max(centre, sharpest cell)


def test_blur_lowers_sharpness_monotonically():
    img = synth_image(2, (1024, 768))
    scores = [
        _q(cv2.GaussianBlur(img, (0, 0), s) if s else img)["sharpness"] for s in (0, 1, 2, 4, 8)
    ]
    assert all(a > b for a, b in zip(scores, scores[1:], strict=False)), scores
    assert scores[0] - scores[-1] > 0.2


def test_darkening_lowers_exposure():
    img = synth_image(3, (1024, 768))
    scores = [
        _q((img.astype(np.float32) * g).astype(np.uint8))["exposure"]
        for g in (1.0, 0.6, 0.35, 0.15)
    ]
    assert all(a > b for a, b in zip(scores, scores[1:], strict=False)), scores


def test_clipping_lowers_exposure():
    img = synth_image(4, (1024, 768))
    blown = np.clip(img.astype(np.int16) + 140, 0, 255).astype(np.uint8)
    assert _q(blown)["exposure"] < _q(img)["exposure"]
    assert _q(blown)["clipped_highlights"] > 0.2


def test_noise_estimate_increases_with_noise():
    img = synth_image(5, (1024, 768))
    rng = np.random.default_rng(0)
    vals = []
    for sigma in (0, 4, 10, 20):
        noisy = np.clip(img.astype(np.float32) + rng.normal(0, sigma, img.shape), 0, 255).astype(
            np.uint8
        )
        vals.append(_q(noisy)["noise_sigma"])
    assert all(a < b for a, b in zip(vals, vals[1:], strict=False)), vals
    assert 10 < vals[-1] < 24  # same ballpark as the injected sigma (clipping biases it low)


def test_resolution_normalised():
    big = synth_image(6, (2048, 1536))
    small = cv2.resize(big, (1024, 768), interpolation=cv2.INTER_AREA)
    a, b = _q(big)["sharpness"], _q(small)["sharpness"]
    assert abs(a - b) < 0.15


def test_face_sharpness_used_when_faces_given():
    img = synth_image(7, (1024, 768))
    box = (0.3, 0.3, 0.3, 0.4)
    q = _q(img, [box])
    assert q["sharpness_face"] is not None
    assert q["sharpness"] == q["sharpness_face"]
    blurred = img.copy()
    x0, y0, x1, y1 = 307, 230, 614, 537
    blurred[y0:y1, x0:x1] = cv2.GaussianBlur(img[y0:y1, x0:x1], (0, 0), 6)
    assert face_sharpness(blurred, box) < face_sharpness(img, box)


def test_tiny_face_box_ignored():
    img = synth_image(8, (640, 480))
    assert face_sharpness(img, (0.5, 0.5, 0.001, 0.001)) is None


def test_off_centre_subject_is_not_blur():
    # detail only in the top-left corner: the centre is flat, the corner cell of the grid is sharp
    img = np.full((768, 1024, 3), 100, np.uint8)
    yy, xx = np.mgrid[0:128, 0:170]
    img[:128, :170][((xx // 16 + yy // 16) % 2) == 0] = 200
    q = _q(img)
    assert q["sharpness_center"] < 0.05
    assert q["sharpness"] > 0.3
    # blurring the whole frame (camera shake, missed focus) still reads as blur
    soft = _q(cv2.GaussianBlur(img, (0, 0), 8))
    assert soft["sharpness"] < q["sharpness"] - 0.2


def test_face_sharpness_weighted_by_face_area():
    img = synth_image(9, (1024, 768))
    big, small = (0.3, 0.3, 0.3, 0.4), (0.85, 0.05, 0.06, 0.08)
    x0, y0, x1, y1 = 870, 38, 932, 100  # blur the small "background" face only
    img[y0:y1, x0:x1] = cv2.GaussianBlur(img[y0:y1, x0:x1], (0, 0), 6)
    s_big, s_small = face_sharpness(img, big), face_sharpness(img, small)
    assert s_big is not None and s_small is not None and s_small < s_big
    q = _q(img, [big, small])
    want = (0.12 * s_big + 0.0048 * s_small) / (0.12 + 0.0048)
    assert abs(q["sharpness_face"] - want) < 1e-4
    assert q["sharpness_face"] > (s_big + s_small) / 2  # the plain mean would be dragged down
