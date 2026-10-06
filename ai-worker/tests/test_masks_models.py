"""Mask tests with real weights (`uv run pytest -m models`; downloads into ai-worker/.models)."""

from __future__ import annotations

import asyncio
from pathlib import Path

import cv2
import numpy as np
import pytest
from PIL import Image

from imagepicker_ai import hw as hwmod
from imagepicker_ai.decode import load_image
from imagepicker_ai.masks import MaskGenerator
from imagepicker_ai.models import Downloader, ModelManager, Registry
from imagepicker_ai.steps.faces import FaceAnalyzer

from .test_masks import iou, params_for, sky_scene, sky_truth

pytestmark = pytest.mark.models

EINSTEIN = "Albert_Einstein_Head.jpg"
CURIE = "Marie_Curie_c1920.jpg"


@pytest.fixture(scope="module")
def gen(models_dir):
    hw = hwmod.detect()
    reg = Registry.load()
    mgr = ModelManager(models_dir, reg, hw)
    g = MaskGenerator(mgr, hw)
    d = Downloader(models_dir)
    for mid in g.models_for_targets(["subject", "person", "skin", "hair", "clothes", "sky"]):
        try:
            d.ensure(reg.get(mid))
        except Exception as e:  # noqa: BLE001
            pytest.skip(f"cannot download {mid}: {e}")
    yield g
    g.shutdown()
    mgr.unload()


def run(g, params, progress=None):
    return asyncio.run(g.generate(params, progress))


def load_mask(path) -> np.ndarray:
    return np.asarray(Image.open(path)).astype(np.float32) / 255.0


def faces_of(g, rgb):
    fa = FaceAnalyzer(str(g.manager.path("yunet")), None)
    return sorted(fa.detect(rgb), key=lambda f: f["bbox"][0])


def two_people(commons, tmp_path, gap: int) -> tuple[Path, np.ndarray]:
    h = 900
    ims = []
    for n in (EINSTEIN, CURIE):
        im = load_image(str(commons(n)), 1024).rgb
        ims.append(cv2.resize(im, (round(im.shape[1] * h / im.shape[0]), h)))
    canvas = np.concatenate([ims[0], np.full((h, gap, 3), 128, np.uint8), ims[1]], axis=1)
    p = tmp_path / f"two_{gap}.jpg"
    Image.fromarray(canvas).save(p, quality=95)
    return p, canvas


@pytest.mark.parametrize("gap", [40, 0])
def test_person_bbox_selects_the_right_person(gen, commons, tmp_path, gap):
    path, canvas = two_people(commons, tmp_path, gap)
    faces = faces_of(gen, canvas)
    assert len(faces) == 2
    cx = [f["bbox"][0] + f["bbox"][2] / 2 for f in faces]
    mid = (cx[0] + cx[1]) / 2
    masks = []
    for f in faces:
        res = run(
            gen,
            {**params_for(path, tmp_path / "m", ["person"], size=1024), "person_bbox": f["bbox"]},
        )
        assert res["skipped"] == {} and res["models"]["person"].startswith("birefnet")
        assert Path(res["masks"]["person"]).name.startswith("12_person_")
        masks.append(load_mask(res["masks"]["person"]))
    assert masks[0].shape == masks[1].shape
    h, w = masks[0].shape
    xs = (np.arange(w) + 0.5) / w
    for i, m in enumerate(masks):
        own = (xs < mid) if i == 0 else (xs >= mid)
        total = m.sum()
        assert total > 0.05 * h * w
        inside = m[:, own].sum() / total
        assert inside > 0.93, f"person {i}: only {inside:.2f} of the mask on its own side"
        # the face itself is covered
        bx, by, bw, bh = faces[i]["bbox"]
        core = m[
            int((by + 0.25 * bh) * h) : int((by + 0.75 * bh) * h),
            int((bx + 0.25 * bw) * w) : int((bx + 0.75 * bw) * w),
        ]
        assert core.mean() > 0.9
    # the two masks are (almost) disjoint
    assert (np.minimum(masks[0], masks[1]).sum() / min(masks[0].sum(), masks[1].sum())) < 0.05


def test_subject_skin_hair_clothes_on_portrait(gen, portrait_path, tmp_path):
    rgb = load_image(str(portrait_path), 1024).rgb
    face = faces_of(gen, rgb)[0]["bbox"]
    events: list[dict] = []
    res = run(
        gen,
        params_for(
            portrait_path, tmp_path / "m", ["subject", "skin", "hair", "clothes"], size=1024
        ),
        events.append,
    )
    assert res["skipped"] == {} and set(res["masks"]) == {"subject", "skin", "hair", "clothes"}
    assert res["models"]["skin"] == "mediapipe-selfie-multiclass"
    assert [e["done"] for e in events if e.get("kind") == "mask"] == [1, 2, 3, 4]
    skin, subject = load_mask(res["masks"]["skin"]), load_mask(res["masks"]["subject"])
    h, w = skin.shape
    bx, by, bw, bh = face
    core = (
        slice(int((by + 0.3 * bh) * h), int((by + 0.8 * bh) * h)),
        slice(int((bx + 0.25 * bw) * w), int((bx + 0.75 * bw) * w)),
    )
    assert skin[core].mean() > 0.6, "skin mask must cover the face region"
    assert subject[core].mean() > 0.9
    assert skin.mean() < subject.mean()  # skin is a subset of the subject
    assert load_mask(res["masks"]["hair"])[core].mean() < 0.3  # not on the face


def test_sky_model_on_synthetic_scene(gen, tmp_path):
    img = tmp_path / "sky.png"
    Image.fromarray(sky_scene(800, 600)).save(img)
    res = run(gen, params_for(img, tmp_path / "m", ["sky"], size=800))
    assert res["models"]["sky"] == "skyseg-u2net"
    m = load_mask(res["masks"]["sky"])
    assert iou(m > 0.5, sky_truth(800, 600)) > 0.85


def test_skipped_when_download_not_allowed(models_dir, tmp_path):
    hw = hwmod.detect("cpu")
    g = MaskGenerator(ModelManager(tmp_path / "none", Registry.load(), hw), hw)
    res = run(g, params_for(tmp_path / "x.jpg", tmp_path / "m", ["subject", "person", "skin"]))
    assert res == {
        "masks": {},
        "models": {},
        "skipped": {
            "subject": "model_unavailable",
            "person": "model_unavailable",
            "skin": "model_unavailable",
        },
    }
    g.shutdown()
