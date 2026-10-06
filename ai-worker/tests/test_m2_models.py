"""M2 steps against real weights (downloaded into ai-worker/.models on first run).

Run with:  uv run pytest -m models
Test photos are public-domain images from Wikimedia Commons fetched at test time (skip offline).
"""

import itertools

import cv2
import numpy as np
import pytest

from imagepicker_ai import hw as hwmod
from imagepicker_ai.decode import load_image
from imagepicker_ai.models import Downloader, ModelManager, Registry
from imagepicker_ai.pipeline import Analyzer
from imagepicker_ai.steps.faces import FaceAnalyzer
from imagepicker_ai.steps.identity import IDENTITY_DIM, IdentityEmbedder
from imagepicker_ai.steps.zeroshot import SCENE_CLASSES

pytestmark = pytest.mark.models

EINSTEIN_A = "Albert_Einstein_Head.jpg"  # 1947
EINSTEIN_B = "Einstein_1921_by_F_Schmutzer_-_restoration.jpg"  # 1921, different sitting
EINSTEIN_C = "Einstein_patentoffice.jpg"  # 1905
CURIE = "Marie_Curie_c1920.jpg"
LINCOLN = "Abraham_Lincoln_O-77_matte_collodion_print.jpg"
BURGER = "Hamburger_(black_bg).jpg"
PIZZA = "Pizza_Margherita_stu_spivack.jpg"
EVEREST = "Mount_Everest_as_seen_from_Drukair2_PLW_edit.jpg"
TURNED = "Albert_Einstein_photo_1921.jpg"  # head turned ~40 degrees


def _ensure(models_dir, *ids):
    reg = Registry.load()
    d = Downloader(models_dir)
    for i in ids:
        try:
            d.ensure(reg.get(i))
        except Exception as e:  # noqa: BLE001
            pytest.skip(f"cannot download {i}: {e}")


@pytest.fixture(scope="module")
def hw_auto():
    return hwmod.detect()


@pytest.fixture
def manager(models_dir, hw_auto):
    m = ModelManager(models_dir, Registry.load(), hw_auto)
    yield m
    m.unload()


@pytest.fixture
def analyzer(manager, hw_auto):
    an = Analyzer(manager, hw_auto, decode_workers=4)
    yield an
    an.shutdown()


def _batch(an, paths, out_dir, **kw):
    items = [{"photo_id": i, "path": str(p)} for i, p in enumerate(paths)]
    return an.analyze_batch({"items": items, "out_dir": str(out_dir), "allow_download": True, **kw})


# ------------------------------------------------------------------ identity


def test_identity_same_person_across_photos_beats_different_people(
    models_dir, manager, hw_auto, commons
):
    _ensure(models_dir, "yunet", "auraface")
    people = {
        "einstein_a": EINSTEIN_A,
        "einstein_b": EINSTEIN_B,
        "einstein_c": EINSTEIN_C,
        "curie": CURIE,
        "lincoln": LINCOLN,
    }
    fa = FaceAnalyzer(manager.path("yunet"))
    ie = IdentityEmbedder(manager, hw_auto)
    emb = {}
    for k, name in people.items():
        rgb = load_image(str(commons(name)), 1024).rgb
        faces = fa.analyze(rgb)
        assert len(faces) >= 1, k
        e = ie.embed_faces(rgb, [f["_kps"] for f in faces])
        assert e.shape == (len(faces), IDENTITY_DIM)
        np.testing.assert_allclose(np.linalg.norm(e, axis=1), 1.0, atol=1e-4)
        emb[k] = e[0]  # faces are sorted by area: [0] is the main subject
    same = [
        float(emb[a] @ emb[b])
        for a, b in itertools.combinations(("einstein_a", "einstein_b", "einstein_c"), 2)
    ]
    diff = [
        float(emb[e] @ emb[o])
        for e in ("einstein_a", "einstein_b", "einstein_c")
        for o in ("curie", "lincoln")
    ]
    assert min(same) > 0.3, same
    assert max(diff) < 0.3, diff
    assert min(same) > max(diff) + 0.15, (same, diff)


# ------------------------------------------------------------------ scene


async def test_scene_sanity_sky_gradient_vs_food_photo(models_dir, tmp_path, analyzer, commons):
    # blue sky gradient over a green field with a bright sun: should read as landscape, not food
    h, w = 480, 640
    sky = np.zeros((h, w, 3), np.uint8)
    t = np.linspace(0, 1, h // 2)[:, None, None]
    sky[: h // 2] = (np.array([30, 90, 220]) * (1 - t) + np.array([200, 225, 255]) * t).astype(
        np.uint8
    )
    sky[h // 2 :] = (60, 140, 50)
    cv2.circle(sky, (500, 90), 30, (255, 245, 200), -1, cv2.LINE_AA)
    gp = tmp_path / "gradient.png"
    cv2.imwrite(str(gp), cv2.cvtColor(sky, cv2.COLOR_RGB2BGR))
    res = await _batch(
        analyzer,
        [gp, commons(BURGER), commons(PIZZA), commons(EVEREST)],
        tmp_path / "o",
        steps=["scene"],
    )
    assert res["skipped_steps"] == [], res["warnings"]
    g, burger, pizza, everest = res["items"]
    for it in res["items"]:
        assert it["scene_type"] in SCENE_CLASSES
        assert set(it["scene_scores"]) == set(SCENE_CLASSES)
        assert sum(it["scene_scores"].values()) == pytest.approx(1.0, abs=0.01)
    assert g["scene_scores"]["landscape"] > burger["scene_scores"]["landscape"]
    assert burger["scene_scores"]["food"] > g["scene_scores"]["food"]
    assert burger["scene_type"] == "food" and pizza["scene_type"] == "food"
    assert everest["scene_type"] == "landscape"
    assert g["scene_type"] != "food"


async def test_embedding_is_computed_once_for_embed_scene_and_iqa(
    models_dir, tmp_path, analyzer, commons, monkeypatch
):
    paths = [commons(n) for n in (BURGER, PIZZA, EVEREST, EINSTEIN_A)]
    seen = []
    orig = analyzer.embedder.embed_prepared

    def spy(prepared):
        seen.append(len(prepared))
        return orig(prepared)

    monkeypatch.setattr(analyzer.embedder, "embed_prepared", spy)
    res = await _batch(analyzer, paths, tmp_path / "o", steps=["embed", "scene", "iqa"])
    assert not res["skipped_steps"], res["warnings"]
    assert sum(seen) == len(paths)  # one image-tower pass per photo, shared by 3 steps
    for it in res["items"]:
        assert "embedding_file" in it and "scene_type" in it and 0 <= it["iqa"] <= 1
        assert it["iqa_model"] == "nima-technical+siglip2-zeroshot"


# ------------------------------------------------------------------ aesthetic / iqa


def _distort(rgb, kind):
    if kind == "blur":
        return cv2.GaussianBlur(rgb, (0, 0), 7)
    rng = np.random.default_rng(0)
    return (rgb.astype(np.float32) + rng.normal(0, 40, rgb.shape)).clip(0, 255).astype(np.uint8)


async def test_iqa_drops_with_blur_and_noise_and_aesthetic_with_blur(
    models_dir, tmp_path, analyzer, commons
):
    names = [EVEREST, BURGER, PIZZA, EINSTEIN_A, CURIE, "Eiffel_Tower_20051010.jpg"]
    clean, blur, noise = [], [], []
    for n in names:
        rgb = load_image(str(commons(n)), 1024).rgb
        for kind, bucket in (("clean", clean), ("blur", blur), ("noise", noise)):
            img = rgb if kind == "clean" else _distort(rgb, kind)
            p = tmp_path / f"{len(bucket)}_{kind}.png"
            cv2.imwrite(str(p), cv2.cvtColor(img, cv2.COLOR_RGB2BGR))
            bucket.append(p)
    allp = clean + blur + noise
    res = await _batch(analyzer, allp, tmp_path / "o", steps=["aesthetic", "iqa"])
    assert not res["skipped_steps"], res["warnings"]
    n = len(names)
    iqa = np.array([i["iqa"] for i in res["items"]])
    aes = np.array([i["aesthetic"] for i in res["items"]])
    assert ((iqa >= 0) & (iqa <= 1)).all() and ((aes >= 0) & (aes <= 1)).all()
    ic, ib, inn = iqa[:n], iqa[n : 2 * n], iqa[2 * n :]
    assert (ib < ic).all(), (ic, ib)  # blur lowers iqa for every photo
    assert inn.mean() < ic.mean() - 0.03, (ic, inn)  # noise lowers it clearly on average
    assert (inn < ic).sum() >= n - 1
    assert aes[n : 2 * n].mean() < aes[:n].mean()  # heavy blur is also less pleasing
    assert res["items"][0]["aesthetic_model"] == "nima-aesthetic"
    assert res["items"][0]["iqa_model"] == "nima-technical+siglip2-zeroshot"


async def test_iqa_nima_only_when_no_embedding_is_computed_on_cpu(models_dir, tmp_path, commons):
    _ensure(models_dir, "nima-aesthetic", "nima-technical")
    hw = hwmod.detect("cpu")
    m = ModelManager(models_dir, Registry.load(), hw)
    an = Analyzer(m, hw, decode_workers=2)
    try:
        res = await _batch(
            an, [commons(EVEREST), commons(BURGER)], tmp_path / "o", steps=["iqa", "aesthetic"]
        )
        assert not res["skipped_steps"]
        assert [i["iqa_model"] for i in res["items"]] == ["nima-technical"] * 2
        assert all(0 <= i["iqa"] <= 1 and 0 <= i["aesthetic"] <= 1 for i in res["items"])
        # CPU cost of both NIMA heads (decode excluded) stays small enough for T0
        t = res["timings"]
        assert (t["aesthetic_s"] + t["iqa_s"]) / 2 < 0.25  # s per image, loose bound
    finally:
        an.shutdown()


# ------------------------------------------------------------------ gaze


def test_gaze_range_and_frontal_vs_turned(models_dir, manager, commons):
    _ensure(models_dir, "yunet", "mediapipe-face-landmarker")
    from imagepicker_ai.steps.faces import mediapipe_available

    if not mediapipe_available():
        pytest.skip("mediapipe not installed")
    fa = FaceAnalyzer(manager.path("yunet"), manager.path("mediapipe-face-landmarker"))
    frontal = fa.analyze(load_image(str(commons(LINCOLN)), 1024).rgb)[0]
    turned = fa.analyze(load_image(str(commons(TURNED)), 1024).rgb)[0]
    for f in (frontal, turned):
        assert f["gaze"] is not None and 0.0 <= f["gaze"] <= 1.0
    assert frontal["gaze"] > 0.5 > turned["gaze"]
    # YuNet-only: no landmarks -> gaze is null
    yo = FaceAnalyzer(manager.path("yunet"), None)
    assert yo.analyze(load_image(str(commons(LINCOLN)), 1024).rgb)[0]["gaze"] is None


# ------------------------------------------------------------------ end to end (contract A)


async def test_standard_and_fast_profiles_end_to_end(models_dir, tmp_path, analyzer, commons):
    paths = [commons(n) for n in (EINSTEIN_A, EINSTEIN_B, BURGER, EVEREST)]
    out = tmp_path / "o"
    fast = await _batch(analyzer, paths, out, profile="fast")
    assert fast["steps"] == ["phash", "quality", "faces"] and fast["profile"] == "fast"
    assert "aesthetic" not in fast["items"][0] and "identity_file" not in fast["items"][0]
    assert not list(out.glob("*.faces.npy"))
    assert all("_kps" not in f for it in fast["items"] for f in it["faces"])

    std = await _batch(analyzer, paths, out, profile="standard")
    assert std["steps"] == [
        "phash",
        "quality",
        "faces",
        "identity",
        "embed",
        "aesthetic",
        "iqa",
        "scene",
    ]
    assert std["skipped_steps"] == [], std["warnings"]
    assert std["models"]["identity"] == "auraface"
    for it in std["items"]:
        assert "error" not in it, it
        assert 0 <= it["aesthetic"] <= 1 and 0 <= it["iqa"] <= 1
        assert it["aesthetic_model"] and it["iqa_model"]
        assert it["scene_type"] in SCENE_CLASSES
        assert it["embedding_file"].endswith(".emb.npy")
        faces = it["faces"]
        if faces:
            arr = np.load(it["identity_file"])
            assert arr.dtype == np.float16 and arr.shape == (len(faces), IDENTITY_DIM)
            np.testing.assert_allclose(
                np.linalg.norm(arr.astype(np.float32), axis=1), 1.0, atol=5e-3
            )
            assert [f["identity_index"] for f in faces] == list(range(len(faces)))
            assert all("_kps" not in f for f in faces)
            assert all(f["gaze"] is None or 0 <= f["gaze"] <= 1 for f in faces)
        else:
            assert it["identity_file"] is None
    assert std["items"][0]["identity_file"] and std["items"][2]["identity_file"] is None
    # two photos of the same person: faces.npy rows are comparable across files
    a = np.load(std["items"][0]["identity_file"]).astype(np.float32)[0]
    b = np.load(std["items"][1]["identity_file"]).astype(np.float32)[0]
    assert float(a @ b) > 0.3
