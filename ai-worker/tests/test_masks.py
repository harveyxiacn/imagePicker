"""`mask.generate`: contract, orientation, sky heuristic, guided filter. No network, no weights."""

from __future__ import annotations

import asyncio
from pathlib import Path

import cv2
import numpy as np
import pytest
from PIL import Image

from imagepicker_ai import hw as hwmod
from imagepicker_ai.errors import InvalidParams
from imagepicker_ai.masks import MaskGenerator, bbox_hash, mask_path
from imagepicker_ai.masks.filters import guided_filter, luminance, refine_alpha
from imagepicker_ai.masks.sky import sky_heuristic
from imagepicker_ai.models import ModelManager, Registry


def sky_scene(w: int = 640, h: int = 480, horizon: float = 0.45, seed: int = 0) -> np.ndarray:
    """Blue gradient sky (lighter at the horizon) over textured green/brown ground."""
    rng = np.random.default_rng(seed)
    hy = int(h * horizon)
    img = np.zeros((h, w, 3), np.uint8)
    t = np.linspace(0, 1, hy, dtype=np.float32)[:, None]
    top, bot = np.array([40, 100, 210], np.float32), np.array([170, 210, 245], np.float32)
    img[:hy] = (top * (1 - t[..., None]) + bot * t[..., None]).astype(np.uint8)
    ground = rng.normal(0, 18, (h - hy, w, 1)) + np.array([70, 110, 45])
    img[hy:] = np.clip(ground, 0, 255).astype(np.uint8)
    cv2.circle(img, (w // 3, int(h * 0.2)), 40, (250, 250, 252), -1, cv2.LINE_AA)  # a cloud
    return img


def sky_truth(w: int = 640, h: int = 480, horizon: float = 0.45) -> np.ndarray:
    t = np.zeros((h, w), bool)
    t[: int(h * horizon)] = True
    return t


def iou(a: np.ndarray, b: np.ndarray) -> float:
    return float((a & b).sum() / max((a | b).sum(), 1))


@pytest.fixture
def generator(tmp_path: Path):
    hw = hwmod.detect("cpu")
    mgr = ModelManager(tmp_path / "empty-models", Registry.load(), hw)
    g = MaskGenerator(mgr, hw)
    yield g
    g.shutdown()


def run(g: MaskGenerator, params: dict, progress=None):
    return asyncio.run(g.generate(params, progress))


def params_for(path: Path, out: Path, targets, **kw):
    return {
        "photo": {"photo_id": 12, "path": str(path), "orientation": kw.pop("orientation", 1)},
        "targets": targets,
        "size": kw.pop("size", 512),
        "out_dir": str(out),
        **kw,
    }


def test_guided_filter_snaps_alpha_to_image_edge():
    h, w, edge = 64, 128, 70
    img = np.zeros((h, w, 3), np.uint8)
    img[:, edge:] = 220
    # a coarse, offset, blurry alpha as produced by a low-resolution network
    coarse = np.zeros((h, w), np.float32)
    coarse[:, edge - 3 :] = 1.0
    coarse = cv2.GaussianBlur(coarse, (0, 0), 4.0)
    out = refine_alpha(coarse, img, radius_frac=0.16, eps=1e-3)
    assert out.dtype == np.float32 and out.min() >= 0 and out.max() <= 1
    # the soft ramp of the network is replaced by a jump exactly at the image edge
    assert (coarse[:, edge] - coarse[:, edge - 1]).max() < 0.15
    assert (out[:, edge] - out[:, edge - 1]).min() > 0.4
    # constant guide: the filter degrades to a plain box blur, never amplifies
    flat = guided_filter(np.full((h, w), 0.5, np.float32), coarse, 3, 1e-3)
    assert np.abs(flat - coarse).max() < 0.3


def test_luminance_range():
    assert luminance(np.full((2, 2, 3), 255, np.uint8)).max() == pytest.approx(1.0, abs=1e-4)


def test_mask_path_and_bbox_hash(tmp_path):
    bbox = [0.1, 0.2, 0.3, 0.4]
    assert bbox_hash(bbox) == bbox_hash([0.10001, 0.2, 0.3, 0.4])
    assert bbox_hash(bbox) != bbox_hash([0.5, 0.2, 0.3, 0.4])
    assert mask_path(tmp_path, 12, "sky", None).name == "12_sky.png"
    assert mask_path(tmp_path, 12, "sky", bbox).name == "12_sky.png"
    assert mask_path(tmp_path, 12, "person", bbox).name == f"12_person_{bbox_hash(bbox)}.png"
    assert mask_path(tmp_path, "a/b", "subject", None).name == "a_b_subject.png"


def test_sky_heuristic_synthetic():
    m = sky_heuristic(sky_scene())
    assert m.shape == (480, 640) and m.dtype == np.float32
    assert iou(m > 0.5, sky_truth()) > 0.92


def test_sky_heuristic_no_sky():
    rng = np.random.default_rng(1)
    ground = np.clip(rng.normal(0, 25, (240, 320, 3)) + np.array([90, 70, 40]), 0, 255)
    assert (sky_heuristic(ground.astype(np.uint8)) > 0.5).mean() < 0.02


def test_contract_shape_skipped_and_progress(generator, tmp_path):
    img = tmp_path / "scene.jpg"
    Image.fromarray(sky_scene(800, 600)).save(img, quality=95)
    events: list[dict] = []
    out = tmp_path / "masks"
    res = run(
        generator,
        params_for(img, out, ["sky", "subject", "skin", "hair", "clothes", "nope"], size=512),
        events.append,
    )
    assert set(res) == {"masks", "models", "skipped"}
    # sky needs no model (heuristic fallback); the rest are skipped, never an error
    assert set(res["masks"]) == {"sky"} and res["models"] == {"sky": "heuristic"}
    assert res["skipped"] == {
        "subject": "model_unavailable",
        "skin": "model_unavailable",
        "hair": "model_unavailable",
        "clothes": "model_unavailable",
        "nope": "unsupported_target",
    }
    p = Path(res["masks"]["sky"])
    assert p == out / "12_sky.png"
    with Image.open(p) as im:
        assert im.mode == "L" and im.size == (512, 384)  # long edge = size, same aspect
    # one active target -> no progress notifications
    assert not [e for e in events if e.get("kind") == "mask"]


def test_size_upscales_small_image(generator, tmp_path):
    img = tmp_path / "small.png"
    Image.fromarray(sky_scene(200, 100)).save(img)
    res = run(generator, params_for(img, tmp_path / "m", ["sky"], size=400))
    with Image.open(res["masks"]["sky"]) as im:
        assert im.size == (400, 200)


def test_orientation_exif6_matches_upright(generator, tmp_path):
    upright = sky_scene(480, 360)
    stored = Image.fromarray(upright).transpose(Image.Transpose.ROTATE_90)  # EXIF 6 = rotate 270
    exif = Image.Exif()
    exif[0x0112] = 6
    img = tmp_path / "rot.jpg"
    stored.save(img, quality=95, exif=exif)
    res = run(generator, params_for(img, tmp_path / "m", ["sky"], size=480, orientation=6))
    m = np.asarray(Image.open(res["masks"]["sky"]))
    assert m.shape == (360, 480)  # upright, not the stored 480x360 rotated frame
    assert iou(m > 127, sky_truth(480, 360)) > 0.9
    # a hint on a file *without* the tag is applied too (Core extracted a preview)
    plain = tmp_path / "plain.jpg"
    Image.fromarray(np.asarray(stored)).save(plain, quality=95)
    res2 = run(generator, params_for(plain, tmp_path / "m2", ["sky"], size=480, orientation=6))
    m2 = np.asarray(Image.open(res2["masks"]["sky"]))
    assert m2.shape == (360, 480) and iou(m2 > 127, sky_truth(480, 360)) > 0.9


@pytest.mark.parametrize(
    "patch",
    [
        {"targets": []},
        {"targets": "sky"},
        {"size": 10},
        {"out_dir": ""},
        {"person_bbox": [0.1, 0.1, 0.0, 0.3]},
        {"person_bbox": [0.1, 0.1]},
        {"photo": {"path": "x.jpg"}},
    ],
)
def test_invalid_params(generator, tmp_path, patch):
    base = params_for(tmp_path / "x.jpg", tmp_path / "m", ["sky"])
    with pytest.raises(InvalidParams):
        run(generator, {**base, **patch})


def test_decode_failure_is_rpc_error(generator, tmp_path):
    from imagepicker_ai.errors import RpcError

    bad = tmp_path / "bad.jpg"
    bad.write_bytes(b"not an image")
    with pytest.raises(RpcError) as ei:
        run(generator, params_for(bad, tmp_path / "m", ["sky"]))
    assert ei.value.kind == "decode_failed"


def test_registry_declares_mask_models():
    reg = Registry.load()
    need = {t: [s.id for s in reg if f"mask.{t}" in s.required_for] for t in ("subject", "person")}
    assert "birefnet-lite" in need["subject"] and need["subject"] == need["person"]
    for t in ("skin", "hair", "clothes"):
        assert [s.id for s in reg if f"mask.{t}" in s.required_for] == [
            "mediapipe-selfie-multiclass"
        ]
    assert [s.id for s in reg if "mask.sky" in s.required_for] == ["skyseg-u2net"]
    for mid in ("birefnet-lite", "birefnet-lite-fp16", "birefnet-lite-512", "birefnet-fp16"):
        s = reg.get(mid)
        assert s.license == "MIT" and not s.noncommercial and s.primary.sha256 and s.size_mb > 0
    for mid in ("mediapipe-selfie-multiclass", "skyseg-u2net"):
        assert reg.get(mid).primary.sha256 and not reg.get(mid).noncommercial
