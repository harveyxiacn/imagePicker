import json

import numpy as np
import pytest
from PIL import Image

from imagepicker_ai.decode import DecodeError, load_image

from .conftest import synth_image


def _save_with_orientation(path, rgb, orientation):
    im = Image.fromarray(rgb)
    exif = Image.Exif()
    exif[0x0112] = orientation
    im.save(path, "JPEG", quality=95, exif=exif)


def test_exif_orientation_applied(tmp_path):
    rgb = synth_image(1, (400, 200))  # landscape 400x200
    p = tmp_path / "rot.jpg"
    _save_with_orientation(p, rgb, 6)  # needs 90deg CW -> portrait 200x400
    d = load_image(str(p), 1024)
    assert (d.orig_width, d.orig_height) == (200, 400)
    assert d.rgb.shape[:2] == (400, 200)


def test_orientation_hint_only_when_file_has_none(tmp_path):
    rgb = synth_image(2, (400, 200))
    p = tmp_path / "plain.jpg"
    Image.fromarray(rgb).save(p, "JPEG")
    assert load_image(str(p), 1024, orientation_hint=6).rgb.shape[:2] == (400, 200)
    q = tmp_path / "tagged.jpg"
    _save_with_orientation(q, rgb, 1)
    assert load_image(str(q), 1024, orientation_hint=6).rgb.shape[:2] == (400, 200)
    r = tmp_path / "tagged3.jpg"
    _save_with_orientation(r, rgb, 3)  # file says 180deg: shape unchanged, hint ignored
    assert load_image(str(r), 1024, orientation_hint=6).rgb.shape[:2] == (200, 400)


def test_resize_to_analysis_size(tmp_path):
    p = tmp_path / "big.png"
    Image.fromarray(synth_image(3, (2000, 1000))).save(p)
    d = load_image(str(p), 800)
    assert max(d.rgb.shape[:2]) == 800
    assert (d.orig_width, d.orig_height) == (2000, 1000)
    assert d.scale == pytest.approx(0.4)
    small = tmp_path / "small.png"
    Image.fromarray(synth_image(3, (300, 200))).save(small)
    assert load_image(str(small), 800).rgb.shape[:2] == (200, 300)  # never upscaled


def test_modes_and_errors(tmp_path):
    p = tmp_path / "gray.png"
    Image.fromarray(np.full((50, 60), 128, np.uint8)).save(p)
    assert load_image(str(p)).rgb.shape == (50, 60, 3)
    rgba = tmp_path / "rgba.png"
    Image.fromarray(np.zeros((20, 20, 4), np.uint8)).save(rgba)
    assert load_image(str(rgba)).rgb.shape == (20, 20, 3)
    junk = tmp_path / "junk.jpg"
    junk.write_bytes(b"not an image")
    with pytest.raises(DecodeError):
        load_image(str(junk))


def test_heic_if_supported(tmp_path):
    pillow_heif = pytest.importorskip("pillow_heif")
    rgb = synth_image(4, (320, 240))
    p = tmp_path / "x.heic"
    try:
        pillow_heif.from_pillow(Image.fromarray(rgb)).save(str(p), quality=80)
    except Exception as e:  # noqa: BLE001
        pytest.skip(f"no HEIC encoder: {e}")
    d = load_image(str(p))
    assert d.rgb.shape[:2] == (240, 320)


async def test_analyzer_classic_steps_without_models(tmp_path, synth_files):
    from imagepicker_ai.hw import detect
    from imagepicker_ai.models import ModelManager, Registry
    from imagepicker_ai.pipeline import Analyzer

    hw = detect("cpu")
    mgr = ModelManager(tmp_path / "m", Registry.load(), hw)
    an = Analyzer(mgr, hw, decode_workers=2)
    seen: list[tuple[str, object, dict[str, float]]] = []
    an.on_timing = lambda kind, key, tm: seen.append((kind, key, tm))  # bench hook
    try:
        items = [{"photo_id": f"p{i}", "path": str(p)} for i, p in enumerate(synth_files)]
        res = await an.analyze_batch(
            {"items": items, "steps": ["phash", "quality"], "out_dir": str(tmp_path / "o")}
        )
        assert [i["photo_id"] for i in res["items"]] == ["p0", "p1", "p2", "p3"]  # order preserved
        j = json.loads((tmp_path / "o" / "p0.analysis.json").read_text())
        assert j["phash"] == res["items"][0]["phash"]
        assert res["timings"]["images_per_s"] > 0
        assert "faces" not in res["items"][0]
        assert sorted(k for kind, k, _ in seen if kind == "item") == ["p0", "p1", "p2", "p3"]
        assert all({"decode", "quality", "phash"} <= set(tm) for _, _, tm in seen)
    finally:
        an.shutdown()


async def test_embedding_hand_off_in_groups_gives_the_same_results(tmp_path, synth_files):
    """The GPU thread gets photos in groups of `embed_flush` (overlapping decode); results and
    `.emb.npy` artefacts must be the same as with one hand-off per request."""
    from imagepicker_ai.hw import detect
    from imagepicker_ai.models import ModelManager, Registry
    from imagepicker_ai.pipeline import Analyzer

    hw = detect("cpu")
    files = synth_files * 3  # 12 photos
    items = [{"photo_id": f"p{i}", "path": str(p)} for i, p in enumerate(files)]

    async def run(flush: int) -> tuple[list[int], list[dict], list[np.ndarray]]:
        an = Analyzer(ModelManager(tmp_path / "m", Registry.load(), hw), hw, decode_workers=3)
        an.embed_flush = flush
        sizes: list[int] = []

        def fake_embed(prepared):  # deterministic per photo, independent of the batch
            sizes.append(len(prepared))
            return np.stack([p.reshape(-1, 3).mean(0) / 255.0 for p in prepared]).astype(np.float32)

        an.embedder.embed_prepared = fake_embed
        an.missing_models = lambda steps: []
        out = tmp_path / f"o{flush}"
        try:
            res = await an.analyze_batch(
                {"items": items, "steps": ["phash", "embed"], "out_dir": str(out)}
            )
        finally:
            an.shutdown()
        embs = [np.load(r["embedding_file"]) for r in res["items"]]
        for r in res["items"]:
            r.pop("embedding_file"), r.pop("analysis_file", None)
        return sizes, res["items"], embs

    sizes_one, items_one, emb_one = await run(32)
    sizes_grp, items_grp, emb_grp = await run(5)
    assert sizes_one == [12] and sizes_grp == [5, 5, 2]
    assert items_one == items_grp
    for a, b in zip(emb_one, emb_grp, strict=True):
        np.testing.assert_array_equal(a, b)
