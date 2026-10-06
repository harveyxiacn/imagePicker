"""`beauty.prepare` / `faces.embed` with real weights (`uv run pytest -m models`)."""

from __future__ import annotations

import asyncio
from pathlib import Path

import cv2
import numpy as np
import pytest
from PIL import Image

from imagepicker_ai.beauty import landmarks as lmk
from imagepicker_ai.decode import load_image
from imagepicker_ai.models import Downloader
from imagepicker_ai.service import WorkerService
from imagepicker_ai.steps.face_search import faces_embed
from imagepicker_ai.steps.faces import FaceAnalyzer

from .test_beauty import paint_spot

pytestmark = pytest.mark.models

OBAMA = "Official_portrait_of_Barack_Obama.jpg"
MICHELLE = "Michelle_Obama_2013_official_portrait.jpg"


@pytest.fixture(scope="module")
def svc(models_dir):
    s = WorkerService(models_dir=str(models_dir), idle_unload_s=0)
    d = Downloader(models_dir)
    for mid in [*s.beauty.required_models(), "auraface"]:
        try:
            d.ensure(s.registry.get(mid))
        except Exception as e:  # noqa: BLE001
            pytest.skip(f"cannot download {mid}: {e}")
    yield s
    s.beauty.shutdown()
    s.masks.shutdown()
    s.analyzer.shutdown()
    s.manager.unload()


def prepare(svc, path, faces=None, size=1024, out=None, orientation=1):
    params = {
        "photo": {"photo_id": 5, "path": str(path), "orientation": orientation},
        "faces": faces or [],
        "size": size,
        "out_dir": str(out),
    }
    return asyncio.run(svc.beauty.prepare(params))


def mask(path) -> np.ndarray:
    return np.asarray(Image.open(path)).astype(np.float32) / 255.0


def detect_faces(svc, rgb) -> list[dict]:
    fa = FaceAnalyzer(str(svc.manager.path("yunet")), None)
    return sorted(fa.detect(rgb), key=lambda f: f["bbox"][0])


def at(m: np.ndarray, x: float, y: float) -> float:
    h, w = m.shape
    return float(m[min(h - 1, int(y * h)), min(w - 1, int(x * w))])


def poly_centre(lm: list, idx: list[int]) -> tuple[float, float]:
    a = np.array(lm)[idx]
    return float(a[:, 0].mean()), float(a[:, 1].mean())


def test_portrait_contract_and_geometry(svc, commons, tmp_path):
    path = commons(OBAMA)
    rgb = load_image(str(path), 1024).rgb
    face = detect_faces(svc, rgb)[0]
    res = prepare(svc, path, [{"face_id": 881, "bbox": face["bbox"]}], out=tmp_path / "o")
    assert res["skipped"] == {}, res["skipped"]
    assert set(res["models"]) >= {"face_landmarks", "pose", "body_mask", "skin_mask", "blemishes"}
    (p,) = res["people"]
    assert p["face_id"] == 881 and len(p["face_landmarks"]) == 478 and len(p["pose"]) == 33
    assert all(len(pt) == 2 and 0 <= pt[0] <= 1 and 0 <= pt[1] <= 1 for pt in p["face_landmarks"])
    assert all(len(k) == 3 for k in p["pose"])
    # PNGs: 8-bit, long edge = size, upright, named per contract
    for key, suffix in (("skin_mask", "skin"), ("body_mask", "body")):
        assert Path(p[key]).name == f"5_881_{suffix}.png"
        im = Image.open(p[key])
        assert im.mode == "L" and max(im.size) == 1024
    skin, body = mask(p["skin_mask"]), mask(p["body_mask"])
    fx, fy, fw, fh = p["face_box"]
    # landmarks sit on the face box; skin on cheeks / forehead, none on eyes / brows / lips
    lm = p["face_landmarks"]
    nose = lm[lmk.NOSE_TIP]
    assert fx < nose[0] < fx + fw and fy < nose[1] < fy + fh
    assert at(skin, *poly_centre(lm, [50, 101])) > 0.8  # cheek (subject's right)
    assert at(skin, *poly_centre(lm, [280, 330])) > 0.8  # cheek (subject's left)
    for name, idx in (
        ("right eye", lmk.RIGHT_EYE),
        ("left eye", lmk.LEFT_EYE),
        ("right brow", lmk.RIGHT_BROW),
        ("left brow", lmk.LEFT_BROW),
        ("lips", lmk.LIPS_OUTER),
        ("mouth", lmk.LIPS_INNER),
    ):
        cx, cy = poly_centre(lm, idx)
        assert at(skin, cx, cy) < 0.1, f"{name} leaks into the skin mask"
    # exact polygon test: nothing inside the (undilated) eye / lip polygons
    h, w = skin.shape
    pts = np.array(lm) * (w, h)
    for idx in (lmk.RIGHT_EYE, lmk.LEFT_EYE, lmk.LIPS_OUTER):
        poly = lmk.polygon_mask(pts, idx, (h, w)).astype(bool)
        assert skin[poly].max() < 0.15
    assert at(body, fx + fw / 2, fy + fh / 2) > 0.95
    assert (skin * (body < 0.15)).max() < 0.05  # skin is part of the body matte
    # the pose belongs to this face: nose keypoint inside the face box
    assert fx < p["pose"][0][0] < fx + fw and fy < p["pose"][0][1] < fy + fh


def two_person_image(commons, tmp_path, names=(OBAMA, MICHELLE)) -> tuple[Path, np.ndarray, float]:
    h = 900
    ims = []
    for n in names:
        im = load_image(str(commons(n)), 1400).rgb
        ims.append(cv2.resize(im, (round(im.shape[1] * h / im.shape[0]), h)))
    canvas = np.concatenate(ims, axis=1)
    p = tmp_path / "two.jpg"
    Image.fromarray(canvas).save(p, quality=95)
    return p, canvas, ims[0].shape[1] / canvas.shape[1]


@pytest.mark.parametrize("swap", [False, True])
def test_two_people_skin_pose_and_body_are_person_specific(svc, commons, tmp_path, swap):
    names = (MICHELLE, OBAMA) if swap else (OBAMA, MICHELLE)
    path, canvas, boundary = two_person_image(commons, tmp_path, names)
    faces = detect_faces(svc, canvas)
    assert len(faces) == 2
    given = [{"face_id": 100 + i, "bbox": f["bbox"]} for i, f in enumerate(faces)]
    res = prepare(svc, path, given, size=1536, out=tmp_path / "o")
    assert res["skipped"] == {}, res["skipped"]
    skins, bodies = [], []
    for i, p in enumerate(res["people"]):
        assert p["face_id"] == 100 + i
        skin, body = mask(p["skin_mask"]), mask(p["body_mask"])
        skins.append(skin)
        bodies.append(body)
        w = skin.shape[1]
        xs = (np.arange(w) + 0.5) / w
        own = (xs < boundary) if i == 0 else (xs >= boundary)
        assert skin.sum() > 0.01 * skin.size
        assert skin[:, own].sum() / skin.sum() > 0.97, f"skin of person {i} leaks onto the other"
        # (the seam of this composite is a dark, gradient-free region shared by both bodies, so
        # allow some of the lower body to land on the neighbour; faces and hands never do)
        assert body[:, own].sum() / body.sum() > 0.88
        ox, oy, ow, oh = res["people"][1 - i]["face_box"]
        other_face = body[
            int((oy + 0.2 * oh) * body.shape[0]) : int((oy + 0.8 * oh) * body.shape[0]),
            int((ox + 0.2 * ow) * w) : int((ox + 0.8 * ow) * w),
        ]
        assert other_face.mean() < 0.05
        fx, fy, fw, fh = p["face_box"]
        # pose: the right body (nose inside this face's box, other person's pose never picked)
        assert len(p["pose"]) == 33
        assert fx < p["pose"][0][0] < fx + fw and fy < p["pose"][0][1] < fy + fh
        assert all(len(set(pt)) for pt in p["pose"])
        lm = np.array(p["face_landmarks"])
        assert fx - 0.02 < lm[:, 0].min() and lm[:, 0].max() < fx + fw + 0.02
    assert np.minimum(skins[0], skins[1]).sum() / min(skins[0].sum(), skins[1].sum()) < 0.01
    assert np.minimum(bodies[0], bodies[1]).sum() / min(bodies[0].sum(), bodies[1].sum()) < 0.05
    # the two poses are different bodies
    assert abs(res["people"][0]["pose"][0][0] - res["people"][1]["pose"][0][0]) > 0.2


def test_detects_faces_itself_and_reports_models(svc, commons, tmp_path):
    path = commons(OBAMA)
    res = prepare(svc, path, [], size=768, out=tmp_path / "o")
    assert res["models"]["faces"] == "yunet"
    (p,) = res["people"]
    assert p["face_id"] is None and len(p["face_landmarks"]) == 478
    assert Path(p["skin_mask"]).name == "5_f0_skin.png"


def test_blemishes_found_on_painted_spots_and_not_on_eyes(svc, commons, tmp_path):
    rgb = load_image(str(commons(OBAMA)), 1024).rgb.copy()
    base_path = tmp_path / "clean.jpg"
    Image.fromarray(rgb).save(base_path, quality=97)
    clean = prepare(svc, base_path, [], size=1024, out=tmp_path / "c")
    lm0 = np.array(clean["people"][0]["face_landmarks"])
    h, w = rgb.shape[:2]
    pts = lm0 * (w, h)
    fw = float(np.ptp(pts[:, 0]))
    # five spots: forehead, both cheeks, chin, beside the nose (inside the skin, away from features)
    anchors = {"forehead": 10, "cheek_r": 205, "cheek_l": 425, "chin": 152, "nose_side": 116}
    spots = []
    for name, i in anchors.items():
        x, y = pts[i]
        if name == "forehead":
            y += 0.04 * fw
        if name == "chin":
            y -= 0.05 * fw
        spots.append((int(x), int(y), int(0.022 * fw)))
    for k, (x, y, r) in enumerate(spots):
        paint_spot(rgb, x, y, r, (110, 55, 45) if k % 2 == 0 else (170, 70, 65))
    painted = tmp_path / "painted.jpg"
    Image.fromarray(rgb).save(painted, quality=97)
    res = prepare(svc, painted, [], size=1024, out=tmp_path / "p")
    p = res["people"][0]
    found = [(x * w, y * h, r * max(w, h)) for x, y, r in p["blemishes"]]
    hits = sum(near_px(found, x, y, 0.9 * r + 3) for x, y, r in spots)
    assert hits >= 4, f"only {hits}/5 painted spots found: {found} vs {spots}"
    assert len(found) <= len(spots) + 6
    # nothing on the eyes / brows / lips
    lmp = np.array(p["face_landmarks"]) * (w, h)
    for idx in (lmk.RIGHT_EYE, lmk.LEFT_EYE, lmk.RIGHT_BROW, lmk.LEFT_BROW, lmk.LIPS_OUTER):
        poly = lmk.polygon_mask(lmp, idx, (h, w)).astype(bool)
        for fx, fy, _ in found:
            assert not poly[int(fy), int(fx)]
    assert len(clean["people"][0]["blemishes"]) <= 4  # a clean official portrait stays quiet


def near_px(found, x, y, tol) -> bool:
    return any((fx - x) ** 2 + (fy - y) ** 2 <= tol**2 for fx, fy, _ in found)


def test_orientation_is_applied(svc, commons, tmp_path):
    up = Image.open(commons(OBAMA)).convert("RGB")
    stored = up.transpose(Image.Transpose.ROTATE_90)  # stored sideways, EXIF 6 rotates it back
    sideways = tmp_path / "sideways.jpg"
    stored.save(sideways, quality=95)
    a = prepare(svc, commons(OBAMA), [], size=768, out=tmp_path / "a")["people"][0]
    b = prepare(svc, sideways, [], size=768, out=tmp_path / "b", orientation=6)["people"][0]
    assert np.allclose(a["face_box"], b["face_box"], atol=0.02)
    la, lb = np.array(a["face_landmarks"]), np.array(b["face_landmarks"])
    assert np.abs(la.mean(0) - lb.mean(0)).max() < 0.01
    ma, mb = mask(a["skin_mask"]), mask(b["skin_mask"])
    assert ma.shape == mb.shape and np.abs(ma - mb).mean() < 0.02
    # without the hint the image would be sideways: no matching face box
    c = prepare(svc, sideways, [], size=768, out=tmp_path / "c", orientation=1)
    assert not c["people"] or not np.allclose(c["people"][0]["face_box"], a["face_box"], atol=0.05)


def test_faces_embed_matches_identity_step(svc, portrait_path, tmp_path):
    out = tmp_path / "id"
    out.mkdir()
    batch = asyncio.run(
        svc.analyzer.analyze_batch(
            {
                "items": [{"photo_id": 1, "path": str(portrait_path), "orientation": 1}],
                "steps": ["identity"],
                "analysis_size": 1024,
                "out_dir": str(out),
            }
        )
    )
    item = batch["items"][0] if "items" in batch else batch["results"][0]
    stored = np.load(item["identity_file"]).astype(np.float32)
    res = asyncio.run(
        faces_embed(svc.analyzer, {"path": str(portrait_path), "orientation": 1}, svc.io_pool)
    )
    assert len(res["faces"]) == len(res["embeddings"]) == len(stored) >= 1
    emb = np.array(res["embeddings"], np.float32)
    assert emb.shape == (len(stored), 512)
    assert np.allclose(np.linalg.norm(emb, axis=1), 1.0, atol=1e-3)
    for i, f in enumerate(res["faces"]):
        assert set(f) == {"bbox", "det_score"} and len(f["bbox"]) == 4
        assert float(emb[i] @ stored[i]) > 0.99
        assert f["bbox"] == item["faces"][i]["bbox"]


def test_faces_embed_orientation_and_empty(svc, commons, tmp_path):
    img = Image.open(commons(OBAMA)).convert("RGB")
    sideways = tmp_path / "s.jpg"
    img.transpose(Image.Transpose.ROTATE_90).save(sideways, quality=95)
    a = asyncio.run(faces_embed(svc.analyzer, {"path": str(commons(OBAMA))}, svc.io_pool))
    b = asyncio.run(
        faces_embed(svc.analyzer, {"path": str(sideways), "orientation": 6}, svc.io_pool)
    )
    assert float(np.array(a["embeddings"][0]) @ np.array(b["embeddings"][0])) > 0.95
    blank = tmp_path / "blank.jpg"
    Image.fromarray(np.full((200, 300, 3), 128, np.uint8)).save(blank)
    assert asyncio.run(faces_embed(svc.analyzer, {"path": str(blank)}, svc.io_pool)) == {
        "faces": [],
        "embeddings": [],
    }
