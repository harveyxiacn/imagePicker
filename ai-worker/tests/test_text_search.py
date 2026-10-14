"""`text.embed`: contract, L2 normalisation, LRU cache, missing models. No weights, no network."""

from __future__ import annotations

import asyncio
from pathlib import Path

import numpy as np
import pytest

from imagepicker_ai import hw as hwmod
from imagepicker_ai.errors import InvalidParams, ModelUnavailable
from imagepicker_ai.models.registry import Registry
from imagepicker_ai.service import WorkerService
from imagepicker_ai.steps import embed as embed_mod
from imagepicker_ai.steps.embed import EMBED_DIM, TEXT_LEN, Embedder
from imagepicker_ai.steps.text_search import (
    CACHE_SIZE,
    MAX_TEXTS,
    TextSearch,
    normalize_query,
    parse,
    text_embed,
)


class FakeTokenizer:
    """Stands in for `tokenizers.Tokenizer`: one id per UTF-8 byte (CJK works), padded to 64."""

    def encode_batch(self, texts: list[str]):
        class Enc:
            def __init__(self, ids: list[int]):
                self.ids = ids

        return [Enc([1 + b for b in t.encode("utf-8")][:TEXT_LEN]) for t in texts]


class FakeSession:
    """Stands in for the ONNX text tower: un-normalised `pooler_output` derived from the ids."""

    def __init__(self):
        self.calls: list[int] = []

    def get_outputs(self):
        class Output:
            name = "pooler_output"

        return [Output()]

    def run(self, _outs, feeds):
        ids = feeds["input_ids"]
        assert ids.dtype == np.int64 and ids.shape[1] == TEXT_LEN
        self.calls.append(ids.shape[0])
        out = np.zeros((ids.shape[0], EMBED_DIM), np.float32)
        for i, row in enumerate(ids):
            rng = np.random.default_rng(int(row.sum()) + 7 * int(row[0]))
            out[i] = rng.normal(size=EMBED_DIM) * 5.0  # deliberately not unit length
        return [out]


class FakeManager:
    def __init__(self, installed: set[str], session: FakeSession):
        self.installed = installed
        self.session = session
        self.ensured: list[str] = []

    def is_installed(self, model_id: str) -> bool:
        return model_id in self.installed

    def acquire(self, model_id, loader):
        assert model_id == embed_mod.TEXT_MODEL
        if model_id not in self.installed:
            raise ModelUnavailable("not installed", [model_id])
        return embed_mod._OrtText(self.session, FakeTokenizer(), model_id)

    def ensure(self, model_id, *_a):
        self.ensured.append(model_id)
        self.installed.add(model_id)
        return Path(".")


def _embedder(session: FakeSession | None = None, installed=None) -> Embedder:
    ids = {embed_mod.TEXT_MODEL} if installed is None else installed
    mgr = FakeManager(ids, session or FakeSession())
    return Embedder(mgr, hwmod.detect("cpu"), backend="onnx")


def test_embedder_text_shape_and_normalisation():
    e = _embedder()
    v = e.embed_text(["a red car", "一只猫在沙发上"])
    assert v.shape == (2, EMBED_DIM) and v.dtype == np.float32
    assert np.allclose(np.linalg.norm(v, axis=1), 1.0, atol=1e-5)
    assert not np.allclose(v[0], v[1])  # Chinese and English produce different vectors


def test_cache_hits_skip_the_model_and_lru_evicts():
    sess = FakeSession()
    ts = TextSearch(_embedder(sess).embed_text, cache_size=2)
    a, hit = ts.embed(["Sunset  beach"])
    assert hit == [False] and ts.model_calls == 1
    b, hit = ts.embed(["sunset beach"])  # whitespace and case do not matter
    assert hit == [True] and ts.model_calls == 1 and np.array_equal(a, b)
    # duplicates inside one request cost one row of one forward pass
    v, hit = ts.embed(["dog", "dog", "sunset beach"])
    assert hit == [False, False, True] and sess.calls[-1] == 1 and np.array_equal(v[0], v[1])
    # capacity 2: a third query evicts the least recently used one ("sunset beach")
    ts.embed(["cat"])
    assert ts.cached("dog") and ts.cached("cat") and not ts.cached("sunset beach")
    assert len(ts._cache) == 2


def test_default_cache_is_bounded():
    assert CACHE_SIZE >= 64
    assert normalize_query("  A\tB \n") == "a b"


def test_parse_validation():
    assert parse({"texts": ["  hello   world "]}) == (["hello world"], False)
    assert parse({"texts": ["x"], "allow_download": True})[1] is True
    for bad in (
        None,
        {},
        {"texts": []},
        {"texts": "x"},
        {"texts": [1]},
        {"texts": ["  "]},
        {"texts": ["x"] * (MAX_TEXTS + 1)},
        {"texts": ["x" * 300]},
    ):
        with pytest.raises(InvalidParams):
            parse(bad)


def test_rpc_returns_unit_vectors_and_cached_flags():
    sess = FakeSession()
    mgr = FakeManager({embed_mod.TEXT_MODEL}, sess)
    emb = Embedder(mgr, hwmod.detect("cpu"), backend="onnx")
    ts = TextSearch(emb.embed_text)

    async def call(texts):
        return await text_embed(ts, mgr, {"texts": texts}, None)

    r = asyncio.run(call(["海边日落", "sunset at the beach"]))
    assert r["model"] == embed_mod.TEXT_MODEL and r["dim"] == EMBED_DIM
    assert r["cached"] == [False, False] and len(r["vectors"]) == 2
    assert all(abs(np.linalg.norm(v) - 1.0) < 1e-4 for v in r["vectors"])
    assert r["logit_scale"] > 100
    r2 = asyncio.run(call(["海边日落"]))
    assert r2["cached"] == [True] and r2["vectors"][0] == r["vectors"][0]
    assert sess.calls == [2]  # the second call never touched the model


def test_missing_model_is_model_unavailable_and_download_flag_fetches(tmp_path):
    sess = FakeSession()
    mgr = FakeManager(set(), sess)
    ts = TextSearch(Embedder(mgr, hwmod.detect("cpu"), backend="onnx").embed_text)
    with pytest.raises(ModelUnavailable) as ei:
        asyncio.run(text_embed(ts, mgr, {"texts": ["cat"]}, None))
    assert ei.value.code == -32010 and ei.value.extra == {"models": [embed_mod.TEXT_MODEL]}
    r = asyncio.run(text_embed(ts, mgr, {"texts": ["cat"], "allow_download": True}, None))
    assert mgr.ensured == [embed_mod.TEXT_MODEL] and len(r["vectors"]) == 1


def test_service_registers_text_embed(tmp_path):
    svc = WorkerService(models_dir=str(tmp_path / "m"), device="cpu", idle_unload_s=0)
    try:
        assert "text.embed" in svc._handlers()
        with pytest.raises(ModelUnavailable):
            asyncio.run(svc.text_embed({"texts": ["a cat"]}, None))
        with pytest.raises(InvalidParams):
            asyncio.run(svc.text_embed({"texts": []}, None))
    finally:
        svc.analyzer.shutdown()
        svc.masks.shutdown()
        svc.beauty.shutdown()


def test_text_model_is_in_the_registry_for_search():
    spec = Registry.load().get(embed_mod.TEXT_MODEL)
    assert "search" in spec.required_for and "embed_text" in spec.task
