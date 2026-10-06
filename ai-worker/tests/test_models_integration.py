"""Tests that need real model weights (downloaded into ai-worker/.models on first run).

Run with:  uv run pytest -m models
"""

import asyncio
import json

import numpy as np
import pytest
from websockets.asyncio.client import connect

from imagepicker_ai import hw as hwmod
from imagepicker_ai.models import Downloader, ModelManager, Registry
from imagepicker_ai.steps.embed import EMBED_DIM, Embedder

from .conftest import synth_image

pytestmark = pytest.mark.models


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


def test_embedding_shape_norm_and_semantics(models_dir, manager, hw_auto, portrait_path):
    _ensure(models_dir, "siglip2-base-fp16", "siglip2-base-text")
    from imagepicker_ai.decode import load_image

    emb = Embedder(manager, hw_auto)
    imgs = [load_image(str(portrait_path), 1024).rgb, synth_image(1), synth_image(2)]
    v = emb.embed_images(imgs)
    assert v.shape == (3, EMBED_DIM) and v.dtype == np.float32
    np.testing.assert_allclose(np.linalg.norm(v, axis=1), 1.0, atol=1e-4)
    # determinism + batch invariance
    v1 = emb.embed_images(imgs[:1])
    assert float(v[0] @ v1[0]) > 0.999

    t = emb.embed_text(
        ["a photo of a man", "a photo of a landscape", "an abstract colorful pattern"]
    )
    assert t.shape == (3, EMBED_DIM)
    np.testing.assert_allclose(np.linalg.norm(t, axis=1), 1.0, atol=1e-4)
    sims = v[0] @ t.T
    assert sims[0] > sims[1], sims  # portrait matches "a man" better than "a landscape"
    # text embedding is case-insensitive (tokenizer input is lower-cased)
    t2 = emb.embed_text(["A PHOTO OF A MAN"])
    assert float(t2[0] @ t[0]) > 0.999


def test_fp16_gpu_matches_fp32_cpu(models_dir, hw_auto, portrait_path):
    if hw_auto.device == "cpu":
        pytest.skip("no accelerator")
    _ensure(models_dir, "siglip2-base", "siglip2-base-fp16")
    from imagepicker_ai.decode import load_image

    img = [load_image(str(portrait_path), 1024).rgb, synth_image(3)]
    out = []
    for dev in ("auto", "cpu"):
        hw = hwmod.detect(dev)
        m = ModelManager(models_dir, Registry.load(), hw)
        e = Embedder(m, hw)
        out.append(e.embed_images(img))
        assert e.active_model == ("siglip2-base-fp16" if dev == "auto" else "siglip2-base")
        m.unload()
    for a, b in zip(out[0], out[1], strict=True):
        assert float(a @ b) > 0.99


def test_torch_fallback_matches_onnx(models_dir, hw_auto, portrait_path):
    pytest.importorskip("transformers")
    pytest.importorskip("torch")
    _ensure(models_dir, "siglip2-base", "siglip2-base-torch")
    from imagepicker_ai.decode import load_image

    img = [load_image(str(portrait_path), 1024).rgb]
    cpu = hwmod.detect("cpu")
    m = ModelManager(models_dir, Registry.load(), cpu)
    onnx_v = Embedder(m, cpu, backend="onnx").embed_images(img)
    m.unload()
    torch_emb = Embedder(m, cpu, backend="torch")
    torch_v = torch_emb.embed_images(img)
    assert torch_emb.active_model == "siglip2-base-torch"
    assert float(onnx_v[0] @ torch_v[0]) > 0.98
    m.unload()


def test_faces_on_portrait(models_dir, manager, portrait_path):
    _ensure(models_dir, "yunet", "mediapipe-face-landmarker")
    from imagepicker_ai.decode import load_image
    from imagepicker_ai.steps.faces import FaceAnalyzer, mediapipe_available

    fa = FaceAnalyzer(manager.path("yunet"), manager.path("mediapipe-face-landmarker"))
    faces = fa.analyze(load_image(str(portrait_path), 1024).rgb)
    assert len(faces) == 1
    f = faces[0]
    x, y, w, h = f["bbox"]
    assert 0 <= x < 1 and 0 <= y < 1 and w > 0.1 and h > 0.1
    assert 0.7 <= f["det_score"] <= 1.0
    assert 0 <= f["sharpness"] <= 1
    if mediapipe_available():
        assert f["landmarks"] is True
        assert f["eyes_open"] > 0.5  # eyes open in the portrait
        assert 0 <= f["smile"] <= 1
        assert abs(f["yaw"]) < 40 and abs(f["pitch"]) < 40 and abs(f["roll"]) < 40
        assert len(f["blendshapes"]) >= 50
        assert "eyeBlinkLeft" in f["blendshapes"]
    # YuNet-only fallback: landmark fields are null
    yo = FaceAnalyzer(manager.path("yunet"), None)
    g = yo.analyze(load_image(str(portrait_path), 1024).rgb)[0]
    assert g["landmarks"] is False
    assert g["eyes_open"] is None and g["smile"] is None and g["yaw"] is None


def test_no_faces_in_synthetic(models_dir, manager):
    _ensure(models_dir, "yunet")
    from imagepicker_ai.steps.faces import FaceAnalyzer

    assert FaceAnalyzer(manager.path("yunet")).analyze(synth_image(5)) == []


async def test_full_analyze_batch_over_rpc(models_dir, tmp_path, portrait_path, synth_files):
    _ensure(models_dir, "yunet", "mediapipe-face-landmarker", "siglip2-base-fp16")
    from imagepicker_ai.service import WorkerService

    svc = WorkerService(models_dir=str(models_dir), token="t", idle_unload_s=0)
    port = await svc.start("127.0.0.1", 0)
    try:
        async with connect(
            f"ws://127.0.0.1:{port}", additional_headers={"Authorization": "Bearer t"}
        ) as ws:
            items = [{"photo_id": 1, "path": str(portrait_path)}] + [
                {"photo_id": 10 + i, "path": str(p)} for i, p in enumerate(synth_files)
            ]
            await ws.send(
                json.dumps(
                    {
                        "jsonrpc": "2.0",
                        "id": 5,
                        "method": "analyze.batch",
                        "params": {
                            "items": items,
                            "steps": ["phash", "quality", "faces", "embed"],
                            "analysis_size": 1024,
                            "out_dir": str(tmp_path / "out"),
                        },
                    }
                )
            )
            notes, res = [], None
            while res is None:
                m = json.loads(await asyncio.wait_for(ws.recv(), 60))
                if m.get("method") == "progress":
                    notes.append(m["params"])
                elif m.get("id") == 5:
                    res = m
            assert "error" not in res, res
            r = res["result"]
            assert notes[-1]["done"] == notes[-1]["total"] == 5
            first = r["items"][0]
            assert len(first["faces"]) == 1 and first["faces"][0]["eyes_open"] is not None
            assert first["sharpness"] == first["quality"]["sharpness_face"]
            emb = np.load(first["embedding_file"])
            assert emb.dtype == np.float16 and emb.shape == (EMBED_DIM,)
            assert abs(float(np.linalg.norm(emb.astype(np.float32))) - 1) < 5e-3
            assert r["models"]["embed"].startswith("siglip2")
            for it in r["items"][1:]:
                assert it["faces"] == []
    finally:
        await svc.stop()


async def test_models_ensure_streams_progress(tmp_path):
    from imagepicker_ai.service import WorkerService

    svc = WorkerService(models_dir=str(tmp_path / "m"), token="t", idle_unload_s=0)
    port = await svc.start("127.0.0.1", 0)
    try:
        async with connect(
            f"ws://127.0.0.1:{port}", additional_headers={"Authorization": "Bearer t"}
        ) as ws:
            await ws.send(
                json.dumps(
                    {
                        "jsonrpc": "2.0",
                        "id": 1,
                        "method": "models.ensure",
                        "params": {"ids": ["yunet", "mediapipe-face-landmarker"]},
                    }
                )
            )
            notes = []
            while True:
                m = json.loads(await asyncio.wait_for(ws.recv(), 120))
                if m.get("method") == "progress":
                    notes.append(m["params"])
                elif m.get("id") == 1:
                    break
            if "error" in m:
                pytest.skip(f"offline: {m['error']}")
            assert notes and all(n["req"] == 1 and n["kind"] == "model.download" for n in notes)
            assert notes[-1]["phase"] == "done"
            assert {x["id"] for x in m["result"]["models"]} == {
                "yunet",
                "mediapipe-face-landmarker",
            }
            await ws.send(json.dumps({"jsonrpc": "2.0", "id": 2, "method": "models.list"}))
            lst = json.loads(await ws.recv())["result"]["models"]
            assert {x["id"] for x in lst if x["installed"]} == {
                "yunet",
                "mediapipe-face-landmarker",
            }
    finally:
        await svc.stop()
