"""Real-model assistant tests (`pytest -m models`): need the `llm-cuda` / `llm` extra, an NVIDIA GPU
for the default CUDA builds, and the model downloads (~6 GB, cached in .models/)."""

from __future__ import annotations

import cv2
import numpy as np
import pytest

from imagepicker_ai.llm.backend import cuda_available, genai_available
from imagepicker_ai.models.download import Downloader
from imagepicker_ai.service import WorkerService

from .assistant_fixtures import COMMANDS, CONTEXT, TOOLS, matches

pytestmark = pytest.mark.models


@pytest.fixture(scope="module")
def svc(models_dir):
    if not genai_available():
        pytest.skip("onnxruntime-genai not installed (uv sync --extra cuda --extra llm-cuda)")
    s = WorkerService(models_dir=str(models_dir), idle_unload_s=0)
    if s.hw.device != "cuda" or not cuda_available():
        pytest.skip("the default assistant models are CUDA builds")
    s.assistant.any_tier = True
    d = Downloader(models_dir)
    for role in ("llm", "vlm"):
        d.ensure(s.assistant.candidates(role)[0])
    yield s
    import asyncio

    asyncio.run(s.stop())


async def test_fifteen_commands_make_valid_tool_calls(svc) -> None:
    ok = 0
    failed = []
    for msg, loc, expected in COMMANDS:
        r = await svc.assistant.plan(
            {"message": msg, "tools": TOOLS, "context": CONTEXT, "locale": loc}
        )
        assert isinstance(r["reply"], str) and isinstance(r["calls"], list)
        good = matches(r["calls"], expected)
        ok += good
        if not good:
            failed.append((msg, r["calls"]))
    assert ok >= 12, f"{ok}/15; failed: {failed}"


async def test_dark_photo_suggests_brighter_exposure(svc, commons, tmp_path) -> None:
    src = cv2.imread(str(commons("Golden_Gate_Bridge_as_seen_from_Battery_East.jpg")))
    dark = np.clip(src.astype(np.float32) * 0.15, 0, 255).astype(np.uint8)
    p = tmp_path / "dark.jpg"
    cv2.imwrite(str(p), dark)
    r = await svc.assistant.suggest({"photo": {"photo_id": 1, "path": str(p)}, "locale": "en"})
    assert r["adjust"]["exposure"] > 0
    assert "underexposed" in r["problems"] and r["reason"]


async def test_describe_returns_caption_and_keywords(svc, commons) -> None:
    p = str(commons("Eiffel_Tower_20051010.jpg"))
    r = await svc.assistant.describe({"photo": {"photo_id": 2, "path": p}, "locale": "en"})
    assert r["caption"].strip() and len(r["keywords"]) >= 3
    zh = await svc.assistant.describe({"photo": {"photo_id": 2, "path": p}, "locale": "zh-CN"})
    assert zh["caption"].strip()
