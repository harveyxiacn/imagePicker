"""`beauty.prepare` / `faces.embed`: contract, geometry helpers, blemish detector. No network, no weights."""

from __future__ import annotations

import asyncio
from pathlib import Path

import cv2
import numpy as np
import pytest
from PIL import Image

from imagepicker_ai import hw as hwmod
from imagepicker_ai.beauty import BeautyPreparer
from imagepicker_ai.beauty import landmarks as lmk
from imagepicker_ai.beauty.blemish import MAX_COUNT, detect_blemishes
from imagepicker_ai.beauty.pose import pick_pose
from imagepicker_ai.beauty.prepare import exclusion_zones, parse_request, skin_from_parts
from imagepicker_ai.errors import InvalidParams, ModelUnavailable, RpcError
from imagepicker_ai.masks import MaskGenerator
from imagepicker_ai.masks.generator import load_upright
from imagepicker_ai.masks.nets import CLS_FACE_SKIN
from imagepicker_ai.models import ModelManager, Registry
from imagepicker_ai.pipeline import Analyzer
from imagepicker_ai.steps.face_search import faces_embed

SKIN = np.array([222, 170, 148], np.float32)


# --------------------------------------------------------------------------- helpers
def circle_pts(cx: float, cy: float, r: float, n: int) -> np.ndarray:
    a = np.linspace(0, 2 * np.pi, n, endpoint=False)
    return np.stack([cx + r * np.cos(a), cy + r * np.sin(a)], 1)


def synthetic_landmarks(w: int = 400) -> np.ndarray:
    """478 points with eyes / brows / lips as simple shapes on a 400 px wide face."""
    pts = np.zeros((lmk.N_FULL, 2))
    pts[lmk.FACE_OVAL] = circle_pts(w / 2, w / 2, w * 0.48, len(lmk.FACE_OVAL))
    pts[lmk.RIGHT_EYE] = circle_pts(w * 0.3, w * 0.38, w * 0.06, len(lmk.RIGHT_EYE))
    pts[lmk.LEFT_EYE] = circle_pts(w * 0.7, w * 0.38, w * 0.06, len(lmk.LEFT_EYE))
    pts[lmk.RIGHT_BROW] = circle_pts(w * 0.3, w * 0.28, w * 0.05, len(lmk.RIGHT_BROW))
    pts[lmk.LEFT_BROW] = circle_pts(w * 0.7, w * 0.28, w * 0.05, len(lmk.LEFT_BROW))
    pts[lmk.LIPS_OUTER] = circle_pts(w * 0.5, w * 0.75, w * 0.1, len(lmk.LIPS_OUTER))
    pts[lmk.NOSTRILS] = circle_pts(w * 0.5, w * 0.58, w * 0.04, len(lmk.NOSTRILS))
    return pts


def skin_patch(w: int = 400, h: int = 400, seed: int = 0) -> np.ndarray:
    rng = np.random.default_rng(seed)
    noise = cv2.GaussianBlur(rng.normal(0, 3.0, (h, w, 3)).astype(np.float32), (0, 0), 1.0)
    low = cv2.resize(rng.normal(0, 4.0, (4, 4, 3)).astype(np.float32), (w, h))
    return np.clip(SKIN + noise + low, 0, 255).astype(np.uint8)


def paint_spot(img: np.ndarray, x: int, y: int, r: int, color: tuple[int, int, int]) -> None:
    layer = np.zeros(img.shape[:2], np.float32)
    cv2.circle(layer, (x, y), r, 1.0, -1, cv2.LINE_AA)
    layer = cv2.GaussianBlur(layer, (0, 0), r * 0.3)
    layer = layer / max(layer.max(), 1e-6)
    img[:] = (img * (1 - layer[..., None]) + np.array(color, np.float32) * layer[..., None]).astype(
        np.uint8
    )


def near(found, x, y, tol) -> bool:
    return any((fx - x) ** 2 + (fy - y) ** 2 <= tol**2 for fx, fy, _ in found)


# --------------------------------------------------------------------------- landmarks
def test_landmark_index_sets_are_valid_loops():
    for idx in (
        lmk.RIGHT_EYE,
        lmk.LEFT_EYE,
        lmk.RIGHT_BROW,
        lmk.LEFT_BROW,
        lmk.LIPS_OUTER,
        lmk.LIPS_INNER,
        lmk.FACE_OVAL,
        lmk.NOSTRILS,
    ):
        assert len(set(idx)) == len(idx) and all(0 <= i < lmk.N_FACE for i in idx)
    assert (len(lmk.RIGHT_EYE), len(lmk.LEFT_EYE)) == (16, 16)
    assert len(lmk.LIPS_OUTER) == len(lmk.LIPS_INNER) == 20 and len(lmk.FACE_OVAL) == 36


def test_feature_mask_covers_polygons_only():
    pts = synthetic_landmarks()
    m = lmk.feature_mask(pts, (400, 400), 400)
    for cx, cy in ((120, 152), (280, 152), (120, 112), (280, 112), (200, 300)):
        assert m[cy, cx] == 1, (cx, cy)
    assert m[200, 200] == 0 and m[360, 200] == 0 and m[10, 10] == 0
    assert 0.03 < m.mean() < 0.25


# --------------------------------------------------------------------------- skin mask
def test_skin_excludes_eyes_brows_and_lips_and_keeps_the_rest():
    n = 400
    pts = synthetic_landmarks(n)
    rgb = np.tile(SKIN.astype(np.uint8), (n, n, 1))
    probs = np.zeros((n, n, 6), np.float32)
    probs[..., CLS_FACE_SKIN] = 1.0
    body = np.ones((n, n), np.float32)
    skin = skin_from_parts(probs, rgb, body, pts, float(np.ptp(pts[:, 0])))
    assert skin.shape == (n, n) and skin.dtype == np.float32
    for cx, cy in ((120, 152), (280, 152), (120, 112), (280, 112), (200, 300)):
        assert skin[cy, cx] < 0.02, f"feature at {(cx, cy)} not removed: {skin[cy, cx]}"
    for cx, cy in ((200, 200), (60, 250), (340, 250), (200, 360), (200, 60)):
        assert skin[cy, cx] > 0.95, f"skin at {(cx, cy)} lost: {skin[cy, cx]}"


def test_skin_is_limited_to_the_persons_matte():
    n = 400
    pts = synthetic_landmarks(n)
    rgb = np.tile(SKIN.astype(np.uint8), (n, n, 1))
    probs = np.zeros((n, n, 6), np.float32)
    probs[..., CLS_FACE_SKIN] = 1.0  # the segmenter sees skin everywhere (two people)
    body = np.zeros((n, n), np.float32)
    body[:, : n // 2] = 1.0  # but this person only owns the left half
    skin = skin_from_parts(probs, rgb, body, pts, float(np.ptp(pts[:, 0])))
    assert skin[250, 60] > 0.9 and skin[250, 340] < 0.02


# --------------------------------------------------------------------------- blemishes
def test_blemish_detector_finds_dark_and_red_spots():
    rgb = skin_patch()
    spots = [
        (100, 190, 6, (120, 70, 55)),
        (300, 210, 5, (205, 90, 85)),
        (210, 330, 7, (130, 80, 60)),
    ]
    for x, y, r, c in spots:
        paint_spot(rgb, x, y, r, c)
    skin = np.ones(rgb.shape[:2], np.float32)
    found = detect_blemishes(rgb, skin, (0, 0, 400, 400))
    for x, y, r, _ in spots:
        assert near(found, x, y, 0.8 * r + 2), f"spot {(x, y)} missed: {found}"
    assert len(found) <= len(spots) + 1
    assert all(2 < r < 25 for _, _, r in found)


def test_blemish_detector_is_quiet_on_clean_skin():
    for seed in range(3):
        rgb = skin_patch(seed=seed)
        assert detect_blemishes(rgb, np.ones((400, 400), np.float32), (0, 0, 400, 400)) == []


def test_blemish_detector_ignores_eyes_brows_lips_hair_and_lines():
    rgb = skin_patch()
    # an eye: dark iris + red-ish lids, a brow, lips, nostril, a hair strand
    cv2.ellipse(rgb, (120, 152), (26, 11), 0, 0, 360, (60, 40, 35), -1, cv2.LINE_AA)
    cv2.circle(rgb, (120, 152), 7, (20, 15, 15), -1, cv2.LINE_AA)
    cv2.ellipse(rgb, (200, 300), (40, 13), 0, 0, 360, (170, 70, 80), -1, cv2.LINE_AA)
    cv2.ellipse(rgb, (200, 232), (7, 4), 0, 0, 360, (70, 40, 35), -1, cv2.LINE_AA)  # nostril
    cv2.line(rgb, (40, 330), (150, 345), (50, 35, 30), 2, cv2.LINE_AA)  # hair strand
    rgb = cv2.GaussianBlur(rgb, (0, 0), 0.8)
    pts = synthetic_landmarks()
    skin = np.ones((400, 400), np.float32)
    skin = skin * (1 - lmk.feature_mask(pts, (400, 400), 400))  # what skin_from_parts delivers
    exc = exclusion_zones(pts, (400, 400), 400)
    found = detect_blemishes(rgb, skin, (0, 0, 400, 400), exclude=exc)
    assert found == [], found
    # even without the skin mask removing the features, the exclusion zones hold
    found = detect_blemishes(rgb, np.ones((400, 400), np.float32), (0, 0, 400, 400), exclude=exc)
    for fx, fy, _ in found:
        assert not exc[int(fy), int(fx)], (fx, fy)
        assert not (30 < fx < 160 and 325 < fy < 350), "hair strand reported"  # lines rejected


def test_blemish_count_is_capped_and_region_is_respected():
    rgb = skin_patch()
    rng = np.random.default_rng(3)
    for _ in range(80):
        paint_spot(rgb, int(rng.integers(20, 380)), int(rng.integers(20, 380)), 5, (130, 75, 60))
    skin = np.ones((400, 400), np.float32)
    assert len(detect_blemishes(rgb, skin, (0, 0, 400, 400))) <= MAX_COUNT
    region = np.zeros((400, 400), np.uint8)
    region[:, 200:] = 1
    for x, _, _ in detect_blemishes(rgb, skin, (0, 0, 400, 400), region=region):
        assert x >= 190


def test_blemish_detector_tiny_face_returns_nothing():
    assert detect_blemishes(skin_patch(40, 40), np.ones((40, 40), np.float32), (0, 0, 40, 40)) == []


# --------------------------------------------------------------------------- pose association
def fake_pose(
    nose: tuple[float, float], scale: float = 0.02, vis: float = 0.9
) -> list[list[float]]:
    pose = [[nose[0], nose[1] + 0.2, 0.0] for _ in range(33)]
    nx, ny = nose
    offs = {
        0: (0, 0),
        2: (-0.3, -0.3),
        5: (0.3, -0.3),
        7: (-0.7, 0),
        8: (0.7, 0),
        9: (-0.2, 0.5),
        10: (0.2, 0.5),
    }
    for i, (dx, dy) in offs.items():
        pose[i] = [nx + dx * scale, ny + dy * scale, vis]
    for i in range(11, 33):
        pose[i] = [nx + (i - 20) * 0.01, ny + 0.1 + i * 0.01, 0.8]
    return pose


def test_pick_pose_chooses_the_body_of_the_face():
    left, right = fake_pose((0.25, 0.3)), fake_pose((0.7, 0.32))
    face_l, face_r = (0.2, 0.25, 0.1, 0.12), (0.65, 0.26, 0.1, 0.12)
    assert pick_pose([left, right], face_l) is left
    assert pick_pose([right, left], face_r) is right
    assert pick_pose([left, right], (0.45, 0.25, 0.1, 0.12)) == []  # between the two: neither
    assert pick_pose([], face_l) == []
    assert pick_pose([left], face_r) == []  # a body that belongs to somebody else


def test_pick_pose_ignores_invisible_head_and_prefers_the_nearest():
    hidden = fake_pose((0.25, 0.3), vis=0.05)
    assert pick_pose([hidden], (0.2, 0.25, 0.1, 0.12)) == []
    near_, far_ = fake_pose((0.26, 0.31)), fake_pose((0.3, 0.33))
    assert pick_pose([far_, near_], (0.2, 0.25, 0.1, 0.12)) is near_


# --------------------------------------------------------------------------- request / contract
def req(**over):
    base = {
        "photo": {"photo_id": 12, "path": "x.jpg", "orientation": 1},
        "faces": [{"face_id": 881, "bbox": [0.1, 0.1, 0.2, 0.2]}],
        "size": 512,
        "out_dir": "out",
    }
    base.update(over)
    return base


def test_parse_request_validation():
    r = parse_request(req())
    assert r["size"] == 512 and r["faces"][0]["face_id"] == 881 and r["allow_download"] is False
    assert parse_request(req(faces=[]))["faces"] == []
    assert parse_request(req())["orientation"] == 1
    for bad in (
        {"photo": {"path": "x"}},
        req(faces=[{"face_id": 1, "bbox": [0, 0, 0, 1]}]),
        req(faces=[{"face_id": 1}]),
        req(faces="no"),
        req(size=10),
        req(out_dir=""),
        req(photo={"photo_id": 1, "path": "x", "orientation": 9}),
    ):
        with pytest.raises(InvalidParams):
            parse_request(bad)


@pytest.fixture
def empty_preparer(tmp_path: Path):
    hw = hwmod.detect("cpu")
    mgr = ModelManager(tmp_path / "none", Registry.load(), hw)
    g = MaskGenerator(mgr, hw)
    yield BeautyPreparer(g), mgr, hw
    g.shutdown()


def write_photo(path: Path, w: int = 320, h: int = 240) -> Path:
    Image.fromarray(skin_patch(w, h)).save(path, quality=92)
    return path


def test_models_missing_gives_empty_geometry_and_skipped(empty_preparer, tmp_path):
    bp, _, _ = empty_preparer
    p = write_photo(tmp_path / "a.jpg")
    params = {
        "photo": {"photo_id": 7, "path": str(p), "orientation": 1},
        "faces": [{"face_id": 881, "bbox": [0.2, 0.2, 0.3, 0.4]}],
        "size": 256,
        "out_dir": str(tmp_path / "o"),
    }
    res = asyncio.run(bp.prepare(params))
    assert set(res) >= {"people", "models", "skipped"}
    (person,) = res["people"]
    assert person == {
        "face_id": 881,
        "face_box": [0.2, 0.2, 0.3, 0.4],
        "face_landmarks": [],
        "pose": [],
        "skin_mask": None,
        "body_mask": None,
        "blemishes": [],
    }
    assert res["models"] == {}
    assert res["skipped"] == {
        "face_landmarks": "model_unavailable",
        "pose": "model_unavailable",
        "body_mask": "model_unavailable",
        "skin_mask": "model_unavailable",
        "blemishes": "model_unavailable",
    }


def test_no_faces_and_no_detector_returns_no_people(empty_preparer, tmp_path):
    bp, _, _ = empty_preparer
    p = write_photo(tmp_path / "a.jpg")
    res = asyncio.run(
        bp.prepare(
            {"photo": {"photo_id": 7, "path": str(p)}, "size": 256, "out_dir": str(tmp_path / "o")}
        )
    )
    assert res["people"] == [] and res["skipped"]["faces"] == "model_unavailable"


def test_undecodable_photo_is_a_decode_error(empty_preparer, tmp_path):
    bp, _, _ = empty_preparer
    bad = tmp_path / "bad.jpg"
    bad.write_bytes(b"nope")
    with pytest.raises(RpcError) as ei:
        asyncio.run(
            bp.prepare(
                {"photo": {"photo_id": 1, "path": str(bad)}, "size": 256, "out_dir": str(tmp_path)}
            )
        )
    assert ei.value.kind == "decode_failed"


def test_load_upright_applies_orientation_and_size(tmp_path):
    img = np.zeros((60, 100, 3), np.uint8)  # wider than tall; red block top-left
    img[:20, :30] = (255, 0, 0)
    p = tmp_path / "o.png"  # no EXIF: the Core's orientation hint is used
    Image.fromarray(img).save(p)
    up = load_upright(str(p), 200, 6)  # EXIF 6 = rotate 90 deg clockwise to display
    assert up.shape[:2] == (200, 120)  # long edge exactly 200, portrait now
    # top-left block of the stored image ends up in the top-right corner
    assert up[10, -10, 0] > 200 and up[10, 10, 0] < 50
    assert load_upright(str(p), 50, None).shape[:2] == (30, 50)


def test_registry_has_pose_landmarker():
    spec = Registry.load().get("mediapipe-pose-landmarker-full")
    assert spec.license == "Apache-2.0" and spec.backend == "mediapipe"
    assert spec.files[0].sha256 and len(spec.files[0].sha256) == 64
    assert spec.files[0].path.endswith(".task")


# --------------------------------------------------------------------------- faces.embed
def test_faces_embed_validation_and_missing_models(tmp_path):
    hw = hwmod.detect("cpu")
    an = Analyzer(ModelManager(tmp_path / "none", Registry.load(), hw), hw)
    try:
        for bad in (None, {}, {"path": ""}, {"path": "x", "orientation": 0}):
            with pytest.raises(InvalidParams):
                asyncio.run(faces_embed(an, bad, an.decode_pool))
        with pytest.raises(ModelUnavailable) as ei:
            asyncio.run(faces_embed(an, {"path": "x.jpg"}, an.decode_pool))
        assert ei.value.code == -32010
    finally:
        an.shutdown()


def test_service_exposes_methods_and_models(tmp_path):
    from imagepicker_ai.service import WorkerService

    svc = WorkerService(models_dir=str(tmp_path / "m"), device="cpu", idle_unload_s=0)
    try:
        assert {"beauty.prepare", "faces.embed"} <= set(svc._handlers())
        needed = svc.beauty.required_models()
        assert "mediapipe-pose-landmarker-full" in needed and "mediapipe-face-landmarker" in needed
        assert any(m.startswith("birefnet") for m in needed)
        listed = asyncio.run(svc.models_list({}, None))
        assert listed["beauty_models"] == needed
    finally:
        svc.beauty.shutdown()
        svc.masks.shutdown()
        svc.analyzer.shutdown()
