"""M2 steps: fast tests (no network, no model weights)."""

import math

import cv2
import numpy as np
import pytest

from imagepicker_ai.errors import InvalidParams
from imagepicker_ai.pipeline import BatchRequest
from imagepicker_ai.steps import PROFILES
from imagepicker_ai.steps.faces import gaze_score, iris_offsets
from imagepicker_ai.steps.identity import (
    ARCFACE_TEMPLATE,
    align_face,
    estimate_similarity,
    kps_from_bbox,
)
from imagepicker_ai.steps.nima import calibrate_aesthetic, calibrate_technical
from imagepicker_ai.steps.zeroshot import (
    IQA_PAIRS,
    SCENE_CLASSES,
    SCENE_PROMPTS,
    PromptBank,
    combine_iqa,
    iqa_zero_shot,
    scene_scores,
)

# ------------------------------------------------------------------ alignment math


def _apply(m: np.ndarray, pts: np.ndarray) -> np.ndarray:
    return pts @ m[:, :2].T + m[:, 2]


def test_similarity_recovers_known_transform():
    theta, scale, t = math.radians(23), 1.7, np.array([40.0, -12.0])
    r = np.array([[math.cos(theta), -math.sin(theta)], [math.sin(theta), math.cos(theta)]])
    # landmarks in a "photo" such that photo -> template is exactly (scale, theta, t)
    photo = np.linalg.solve(scale * r, (ARCFACE_TEMPLATE - t).T).T
    m = estimate_similarity(photo)
    np.testing.assert_allclose(m[:, :2], scale * r, atol=1e-6)
    np.testing.assert_allclose(m[:, 2], t, atol=1e-5)
    np.testing.assert_allclose(_apply(m, photo), ARCFACE_TEMPLATE, atol=1e-5)


def test_similarity_is_least_squares_under_noise_and_never_a_reflection():
    rng = np.random.default_rng(0)
    photo = ARCFACE_TEMPLATE * 3.0 + np.array([200.0, 100.0]) + rng.normal(0, 0.8, (5, 2))
    m = estimate_similarity(photo)
    a = m[:, :2]
    assert np.linalg.det(a) > 0  # rotation + scale only
    assert abs(a[0, 0] - a[1, 1]) < 1e-9 and abs(a[0, 1] + a[1, 0]) < 1e-9  # similarity structure
    assert np.abs(_apply(m, photo) - ARCFACE_TEMPLATE).max() < 2.0
    # a mirrored landmark set must still give a proper rotation (no reflection)
    mirrored = photo * np.array([-1.0, 1.0])
    assert np.linalg.det(estimate_similarity(mirrored)[:, :2]) > 0


def test_similarity_rejects_degenerate_input():
    with pytest.raises(ValueError):
        estimate_similarity(np.zeros((5, 2)))
    with pytest.raises(ValueError):
        estimate_similarity(np.zeros((3, 3)))


def test_align_face_puts_landmarks_on_the_template():
    # a synthetic photo with 5 bright dots at the (rotated, scaled) landmark positions
    h, w = 480, 640
    theta, scale, t = math.radians(-15), 0.6, np.array([210.0, 150.0])
    r = np.array([[math.cos(theta), -math.sin(theta)], [math.sin(theta), math.cos(theta)]])
    kps = ARCFACE_TEMPLATE @ (scale * r).T + t
    img = np.zeros((h, w, 3), np.uint8)
    for x, y in kps:
        cv2.circle(img, (int(round(x)), int(round(y))), 3, (255, 255, 255), -1, cv2.LINE_AA)
    out = align_face(img, kps)
    assert out.shape == (112, 112, 3)
    # the dots must now sit on the ArcFace template points
    for x, y in ARCFACE_TEMPLATE:
        patch = out[int(y) - 2 : int(y) + 3, int(x) - 2 : int(x) + 3].mean()
        assert patch > 60, (x, y, patch)
    assert out[5:15, 5:15].max() < 40  # corners stay dark
    # identity transform: template-shaped landmarks leave the content in place
    same = estimate_similarity(ARCFACE_TEMPLATE)
    np.testing.assert_allclose(same, np.array([[1, 0, 0], [0, 1, 0]]), atol=1e-9)


def test_kps_from_bbox_is_a_sane_fallback():
    k = kps_from_bbox([0.25, 0.25, 0.2, 0.3], 1000, 800)
    assert k.shape == (5, 2)
    assert k[0, 0] < k[1, 0] and k[0, 1] < k[2, 1] < k[3, 1]  # eyes above nose above mouth
    assert k[:, 0].min() > 200 and k[:, 0].max() < 500


# ------------------------------------------------------------------ profiles / request parsing


def _items():
    return [{"photo_id": 1, "path": "x.jpg"}]


def test_profile_to_steps_mapping():
    assert PROFILES["fast"] == ("phash", "quality", "faces")
    assert PROFILES["standard"] == (
        "phash",
        "quality",
        "faces",
        "identity",
        "embed",
        "aesthetic",
        "iqa",
        "scene",
    )
    fast = BatchRequest.parse({"items": _items(), "profile": "fast"})
    assert fast.steps == ["phash", "quality", "faces"] and fast.profile == "fast"
    std = BatchRequest.parse({"items": _items(), "profile": "standard", "out_dir": "o"})
    assert std.steps == list(PROFILES["standard"]) and std.profile == "standard"
    # no profile, no steps -> the standard profile (contract default)
    default = BatchRequest.parse({"items": _items(), "out_dir": "o"})
    assert default.steps == list(PROFILES["standard"]) and default.profile == "standard"


def test_explicit_steps_win_over_profile():
    r = BatchRequest.parse({"items": _items(), "profile": "standard", "steps": ["phash", "iqa"]})
    assert r.steps == ["phash", "iqa"] and r.profile is None


def test_identity_implies_faces_and_needs_out_dir():
    r = BatchRequest.parse({"items": _items(), "steps": ["identity"], "out_dir": "o"})
    assert r.steps == ["faces", "identity"]
    with pytest.raises(InvalidParams):
        BatchRequest.parse({"items": _items(), "steps": ["identity"]})
    with pytest.raises(InvalidParams):
        BatchRequest.parse({"items": _items(), "profile": "bogus"})
    # iqa is a step of its own now (no longer an alias of quality)
    assert BatchRequest.parse({"items": _items(), "steps": ["iqa"]}).steps == ["iqa"]


async def test_missing_models_skip_optional_steps(tmp_path, synth_files):
    from imagepicker_ai.hw import detect
    from imagepicker_ai.models import ModelManager, Registry
    from imagepicker_ai.pipeline import Analyzer

    hw = detect("cpu")
    an = Analyzer(ModelManager(tmp_path / "m", Registry.load(), hw), hw, decode_workers=2)
    try:
        items = [{"photo_id": "a", "path": str(synth_files[0])}]
        res = await an.analyze_batch(
            {"items": items, "steps": ["phash", "aesthetic", "iqa", "scene"]}
        )
        assert res["steps"] == ["phash"]
        assert res["skipped_steps"] == ["aesthetic", "iqa", "scene"]
        assert len(res["warnings"]) >= 3
        assert "aesthetic" not in res["items"][0] and "scene_type" not in res["items"][0]
    finally:
        an.shutdown()


# ------------------------------------------------------------------ scene zero-shot maths


def _unit(rng, n, d=16):
    v = rng.normal(size=(n, d)).astype(np.float32)
    return v / np.linalg.norm(v, axis=1, keepdims=True)


def test_scene_classes_are_the_contract_classes():
    assert SCENE_CLASSES == (
        "portrait",
        "group",
        "landscape",
        "food",
        "architecture",
        "night",
        "pet",
        "other",
    )
    assert set(SCENE_PROMPTS) == set(SCENE_CLASSES)
    assert all(len(p) >= 3 for p in SCENE_PROMPTS.values())  # several prompts per class


def test_scene_scores_softmax_and_ranking():
    rng = np.random.default_rng(1)
    classes = _unit(rng, 8)
    noise = _unit(rng, 8) * 0.05
    imgs = classes + noise  # image i looks like class i
    p = scene_scores(imgs, classes)
    assert p.shape == (8, 8)
    np.testing.assert_allclose(p.sum(axis=1), 1.0, atol=1e-5)
    assert (p.argmax(axis=1) == np.arange(8)).all()
    assert (p >= 0).all() and (p <= 1).all()


def test_iqa_zero_shot_direction():
    rng = np.random.default_rng(2)
    good, bad = _unit(rng, 3), _unit(rng, 3)
    e_good = good.mean(0, keepdims=True)
    e_bad = bad.mean(0, keepdims=True)
    hi = iqa_zero_shot(e_good, good, bad)[0]
    lo = iqa_zero_shot(e_bad, good, bad)[0]
    assert 0 <= lo < hi <= 1
    assert combine_iqa(0.4, 0.8) == pytest.approx(0.6)
    assert combine_iqa(None, 0.8) == pytest.approx(0.8)
    assert combine_iqa(None, None) is None


def test_prompt_bank_caches_text_embeddings_on_disk(tmp_path):
    calls = []

    def fake_text(texts):
        calls.append(len(texts))
        rng = np.random.default_rng(len(texts))
        return _unit(rng, len(texts), 32)

    bank = PromptBank(tmp_path / "c")
    assert not bank.cached()
    d = bank.get(fake_text)
    n_prompts = sum(len(v) for v in SCENE_PROMPTS.values()) + 2 * len(IQA_PAIRS)
    assert calls == [n_prompts]  # one single batched text pass
    assert d["scene"].shape == (8, 32)
    np.testing.assert_allclose(np.linalg.norm(d["scene"], axis=1), 1.0, atol=1e-5)
    assert d["iqa_good"].shape == (len(IQA_PAIRS), 32)
    # a new bank (new process) reads the cache: the text tower is not touched again
    bank2 = PromptBank(tmp_path / "c")
    assert bank2.cached()
    d2 = bank2.get(lambda t: pytest.fail("text tower must not run on a cache hit"))
    np.testing.assert_allclose(d2["scene"], d["scene"])
    assert calls == [n_prompts]


# ------------------------------------------------------------------ calibration


def test_calibration_is_monotone_and_in_range():
    raws = np.linspace(1, 10, 50)
    for fn in (calibrate_aesthetic, calibrate_technical):
        v = np.array([fn(r) for r in raws])
        assert (np.diff(v) > 0).all() and v.min() > 0 and v.max() < 1
    assert calibrate_aesthetic(5.0) == pytest.approx(0.5)
    assert calibrate_technical(5.0) == pytest.approx(0.5)
    assert calibrate_aesthetic(6.0) > 0.8 > 0.2 > calibrate_aesthetic(4.0)


# ------------------------------------------------------------------ gaze


def _landmarks(iris_dx=0.0, iris_dy=0.0, width=40.0):
    """478 synthetic MediaPipe landmarks: two eyes `width` px wide, iris shifted by (dx, dy)*width."""
    pts = np.zeros((478, 2))
    # (corner A, corner B, upper lid, lower lid, iris) for the two eyes
    for (a, b, up, lo, iris), cx in (
        ((33, 133, 159, 145, 468), 100.0),
        ((362, 263, 386, 374, 473), 200.0),
    ):
        pts[a] = (cx - width / 2, 100)
        pts[b] = (cx + width / 2, 100)
        pts[up] = (cx, 100 - width * 0.12)
        pts[lo] = (cx, 100 + width * 0.12)
        pts[iris] = (cx + iris_dx * width, 100 + iris_dy * width)
    return pts


def test_gaze_centred_iris_frontal_head_is_high():
    g = gaze_score(_landmarks(), yaw=0.0, pitch=0.0, eyes_open=0.95)
    assert g is not None and g > 0.95


def test_gaze_decreases_with_iris_offset_and_head_pose():
    base = gaze_score(_landmarks(), 0, 0, 0.9)
    side = gaze_score(_landmarks(iris_dx=0.25), 0, 0, 0.9)
    up = gaze_score(_landmarks(iris_dy=-0.2), 0, 0, 0.9)
    turned = gaze_score(_landmarks(), 45, 0, 0.9)
    down = gaze_score(_landmarks(), 0, 35, 0.9)
    assert base > side and base > up and base > turned and base > down
    assert side < 0.3 and turned < 0.2
    # symmetric in the sign of the offset / yaw
    assert gaze_score(_landmarks(iris_dx=-0.25), 0, 0, 0.9) == pytest.approx(side, rel=1e-6)
    assert gaze_score(_landmarks(), -45, 0, 0.9) == pytest.approx(turned, rel=1e-6)


def test_gaze_closed_eyes_and_range_and_degenerate():
    assert gaze_score(_landmarks(), 0, 0, 0.05) == pytest.approx(0.0, abs=1e-6)
    for dx in (-0.5, -0.1, 0, 0.3, 1.0):
        for yaw in (-90, -20, 0, 70):
            g = gaze_score(_landmarks(iris_dx=dx), yaw, 10, 0.8)
            assert g is not None and 0.0 <= g <= 1.0
    assert iris_offsets(_landmarks(width=1.0)) is None
    assert gaze_score(_landmarks(width=1.0), 0, 0, 0.9) is None
    # a pitch reported around 180 deg (matrix convention) is treated like ~0
    assert gaze_score(_landmarks(), 0, 178.0, 0.9) == pytest.approx(
        gaze_score(_landmarks(), 0, -2.0, 0.9), rel=1e-6
    )
