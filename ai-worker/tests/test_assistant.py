"""llm.plan / vlm.suggest / vlm.describe without any model: prompt construction, JSON extraction and
repair, clamping, and the three methods against injected stub generators."""

from __future__ import annotations

import json
import time
from pathlib import Path
from typing import Any

import cv2
import numpy as np
import pytest

from imagepicker_ai import errors
from imagepicker_ai.errors import InvalidParams, RpcError
from imagepicker_ai.hw import HardwareInfo
from imagepicker_ai.llm import Assistant
from imagepicker_ai.llm import service as llm_service
from imagepicker_ai.llm.adjust import ADJUST_RANGES, adjust_schema, clamp_adjust
from imagepicker_ai.llm.backend import GenerationTimeout
from imagepicker_ai.llm.facts import (
    baseline_adjust,
    image_facts,
    measured_problems,
    merge_adjust,
)
from imagepicker_ai.llm.jsonx import JsonExtractError, extract_json
from imagepicker_ai.llm.prompts import (
    build_plan_messages,
    build_suggest_prompt,
    build_system_prompt,
    translate_schema,
    usable_examples,
)
from imagepicker_ai.llm.schema import (
    check_plan,
    grammar_ready,
    parse_tools,
    plan_schema,
    prune_invented,
    validate,
)
from imagepicker_ai.models.manager import ModelManager
from imagepicker_ai.models.registry import Registry
from imagepicker_ai.service import WorkerService

from .assistant_fixtures import CONTEXT, TOOLS, matches

TOOL_LIST = parse_tools(TOOLS)


# ---------------------------------------------------------------------------- JSON extraction
@pytest.mark.parametrize(
    "text",
    [
        '{"a": 1}',
        'Sure! Here you go:\n```json\n{"a": 1}\n```\nHope that helps',
        '<think>hmm {"x": 0}</think>\n{"a": 1}',
        '{"a": 1,}',
        "{'a': 1}",
        '{"a": 1',  # truncated
        '{"a": 1, "b":',  # truncated after a key
        '“{"a": 1}',  # junk in front
    ],
)
def test_extract_json_repairs(text: str) -> None:
    got = extract_json(text)
    assert got["a"] == 1


def test_extract_json_nested_truncation_and_literals() -> None:
    got = extract_json('{"calls": [{"tool": "x", "args": {"ok": True, "n": None}}, {"tool": "y"')
    assert got["calls"][0]["args"] == {"ok": True, "n": None}
    assert got["calls"][1]["tool"] == "y"


@pytest.mark.parametrize("text", ["", "   ", "no json here", "[1, 2, 3]"])
def test_extract_json_failures(text: str) -> None:
    with pytest.raises(JsonExtractError):
        extract_json(text)  # a list is not the expected object


# ---------------------------------------------------------------------------- schemas
def test_parse_tools_accepts_common_shapes() -> None:
    tools = parse_tools(
        [
            {"name": "a", "description": "d", "parameters": {"type": "object", "properties": {}}},
            {"type": "function", "function": {"name": "b", "parameters": {"type": "object"}}},
            {
                "name": "c",
                "input_schema": {"type": "object", "properties": {"x": {"type": "integer"}}},
            },
            {"title": "d", "type": "object", "properties": {"y": {"type": "string"}}},
        ]
    )
    assert [t.name for t in tools] == ["a", "b", "c", "d"]
    assert tools[3].schema["properties"] == {"y": {"type": "string"}}


@pytest.mark.parametrize(
    "bad", [None, [], "x", [1], [{"name": "bad name"}], [{"name": "a"}, {"name": "a"}]]
)
def test_parse_tools_rejects(bad: Any) -> None:
    with pytest.raises(InvalidParams):
        parse_tools(bad)


def test_validate_basics() -> None:
    sc = {
        "type": "object",
        "properties": {
            "n": {"type": "integer", "minimum": 1, "maximum": 3},
            "e": {"enum": ["a", "b"]},
            "l": {"type": "array", "items": {"type": "string"}, "maxItems": 2},
            "o": {"anyOf": [{"type": "string"}, {"type": "null"}]},
        },
        "required": ["n"],
        "additionalProperties": False,
    }
    assert validate(sc, {"n": 2, "e": "a", "l": ["x"], "o": None}) == []
    assert validate(sc, {}) and validate(sc, {"n": 4}) and validate(sc, {"n": True})
    assert validate(sc, {"n": 1, "e": "z"}) and validate(sc, {"n": 1, "l": ["a", "b", "c"]})
    assert validate(sc, {"n": 1, "extra": 1}) and validate(sc, {"n": 1, "o": 3})


def test_plan_schema_embeds_every_tool_and_closes_objects() -> None:
    sc = plan_schema(TOOL_LIST)
    branches = sc["properties"]["calls"]["items"]["anyOf"]
    assert [b["properties"]["tool"]["const"] for b in branches] == [t.name for t in TOOL_LIST]
    rating = next(b for b in branches if b["properties"]["tool"]["const"] == "set_rating")
    args = rating["properties"]["args"]
    assert args["additionalProperties"] is False
    assert "description" not in json.dumps(sc)  # annotations stripped for the grammar engine
    # property names that collide with annotation keywords survive
    g = grammar_ready({"type": "object", "properties": {"title": {"type": "string", "title": "T"}}})
    assert list(g["properties"]) == ["title"] and "title" not in g["properties"]["title"]


def test_check_plan_validates_against_tools() -> None:
    plan = {
        "reply": " ok ",
        "calls": [
            {"tool": "set_rating", "args": {"selection": "current_filter", "rating": 5}},
            {"tool": "set_rating", "args": {"selection": "current_filter", "rating": 9}},
            {"tool": "nuke", "args": {}},
            "junk",
        ],
    }
    reply, calls, problems = check_plan(plan, TOOL_LIST)
    assert reply == "ok"
    assert [c["args"]["rating"] for c in calls] == [5]
    assert len(problems) == 3
    assert check_plan([], TOOL_LIST)[2]


def test_prune_invented_optional_strings() -> None:
    calls = [
        {
            "tool": "export",
            "args": {"selection": "current_filter", "preset": "wechat", "dest": "small"},
        }
    ]
    out, notes = prune_invented(calls, TOOL_LIST, "export for wechat", {})
    assert "dest" not in out[0]["args"] and out[0]["args"]["preset"] == "wechat" and notes
    out, notes = prune_invented(calls, TOOL_LIST, "export to small", {})
    assert out[0]["args"]["dest"] == "small" and not notes


# ---------------------------------------------------------------------------- prompts
def test_system_prompt_embeds_tools_and_context() -> None:
    sp = build_system_prompt(TOOL_LIST, CONTEXT, "zh-CN")
    for t in TOOL_LIST:
        assert f"- {t.name}(" in sp
    assert "rating: int 0..5|null" in sp and '"xiaohongshu"' in sp
    assert "[101, 102, 103]" in sp and "42" in sp and "Simplified Chinese" in sp
    assert "淘汰" in sp  # zh glossary


def test_few_shot_zh_and_en_and_filtered_by_tools() -> None:
    msgs = build_plan_messages("hello", TOOL_LIST, CONTEXT, "en")
    assert msgs[0]["role"] == "system" and msgs[-1] == {"role": "user", "content": "hello"}
    users = [m["content"] for m in msgs[1:-1] if m["role"] == "user"]
    assert any(any("一" <= ch <= "鿿" for ch in u) for u in users)
    assert any(u.isascii() for u in users)
    for m in msgs[2:-1:2]:  # every assistant turn is a valid plan
        _, _, problems = check_plan(json.loads(m["content"]), TOOL_LIST)
        assert problems == []
    only = parse_tools([TOOLS[1]])  # set_rating only
    assert {c["tool"] for e in usable_examples(only) for c in e.calls} <= {"set_rating"}


def test_suggest_prompt_carries_measurements_and_ranges() -> None:
    p = build_suggest_prompt("- brightness: dark", "en", {"exposure": 1.0})
    assert "brightness: dark" in p and '"exposure":1.0' in p and "-1500 to 1500" in p
    assert "underexposed" in p


def test_translate_schema_forces_cjk_for_zh() -> None:
    sc = translate_schema({"caption": "x", "keywords": ["a", "b", "c"]}, "zh-CN")
    assert "pattern" in sc["properties"]["caption"]
    assert "pattern" in sc["properties"]["keywords"]["items"]
    assert "pattern" not in translate_schema({"caption": "x"}, "fr")["properties"]["caption"]


# ---------------------------------------------------------------------------- adjust
def test_clamp_adjust_ranges_and_garbage() -> None:
    out = clamp_adjust(
        {
            "exposure": 99,
            "temp": -9999,
            "contrast": "50",
            "tint": float("nan"),
            "clarity": True,
            "x": 1,
        }
    )
    assert out["exposure"] == 5.0 and out["temp"] == -3000 and out["contrast"] == 50
    assert out["tint"] == 0 and out["clarity"] == 0 and "x" not in out
    assert set(ADJUST_RANGES) <= set(out) and out["source"] == "ai_vlm@1"
    assert clamp_adjust(None)["exposure"] == 0.0
    props = adjust_schema()["properties"]
    assert set(props) == set(ADJUST_RANGES) and props["exposure"]["maximum"] == 3.0


def _solid(v: int, shape=(120, 160)) -> np.ndarray:
    return np.full((*shape, 3), v, np.uint8)


def test_facts_baseline_and_problems() -> None:
    dark = image_facts(_solid(20))
    assert baseline_adjust(dark)["exposure"] > 1 and "underexposed" in measured_problems(dark)
    bright = image_facts(_solid(245))
    assert baseline_adjust(bright)["exposure"] < -1 and "overexposed" in measured_problems(bright)
    ok = np.clip(np.linspace(30, 220, 160)[None, :, None] * np.ones((120, 1, 3)), 0, 255).astype(
        np.uint8
    )
    f = image_facts(ok)
    assert "exposure" not in baseline_adjust(f) and "underexposed" not in measured_problems(f)
    blue = np.dstack([_solid(60)[..., 0], _solid(110)[..., 0], _solid(200)[..., 0]])
    assert "color_cast_cool" in measured_problems(image_facts(blue))
    assert baseline_adjust(image_facts(blue))["temp"] > 0


def test_merge_adjust_measured_wins_and_filler_dropped() -> None:
    vlm = clamp_adjust(
        {
            "exposure": -2.5,
            "contrast": 10,
            "shadows": 10,
            "clarity": 25,
            "highlights": -40,
            "temp": 200,
        }
    )
    out = merge_adjust({"exposure": 1.5, "highlights": -30}, vlm)
    assert out["exposure"] == 1.5 and out["highlights"] == -30
    assert out["contrast"] == 0 and out["shadows"] == 0 and out["temp"] == 0
    assert out["clarity"] == 25


# ---------------------------------------------------------------------------- fake backends
class FakeText:
    def __init__(self, *outputs: Any):
        self.outputs = list(outputs)
        self.calls: list[dict[str, Any]] = []

    def generate(self, messages, schema, max_tokens, deadline, cancel=None):  # noqa: ANN001
        self.calls.append({"messages": messages, "schema": schema, "max_tokens": max_tokens})
        out = self.outputs.pop(0)
        if isinstance(out, Exception):
            raise out
        return out if isinstance(out, str) else json.dumps(out, ensure_ascii=False)


class FakeVision:
    def __init__(self, *outputs: Any):
        self.outputs = list(outputs)
        self.calls: list[dict[str, Any]] = []

    def generate(self, rgb, prompt, schema, max_tokens, deadline, cancel=None):  # noqa: ANN001
        self.calls.append({"shape": rgb.shape, "prompt": prompt, "schema": schema})
        out = self.outputs.pop(0)
        return out if isinstance(out, str) else json.dumps(out)


@pytest.fixture
def hw() -> HardwareInfo:
    return HardwareInfo(
        "test", "cpu", 4, 8, 16000, tier="T3", device="cuda", providers=["CUDAExecutionProvider"]
    )


@pytest.fixture
def mgr(tmp_path: Path, hw: HardwareInfo) -> ModelManager:
    return ModelManager(tmp_path / "models", Registry.load(), hw)


def make(mgr, hw, text=None, vision=None) -> Assistant:
    return Assistant(mgr, hw, text_backend=text, vision_backend_=vision)


def plan_params(message: str, **kw: Any) -> dict[str, Any]:
    return {"message": message, "tools": TOOLS, "context": CONTEXT, "locale": "en", **kw}


async def test_plan_returns_validated_calls(mgr, hw) -> None:
    good = {
        "calls": [{"tool": "set_rating", "args": {"selection": "current_filter", "rating": 5}}],
        "reply": "Rating them 5 stars.",
    }
    fake = FakeText(good)
    r = await make(mgr, hw, text=fake).plan(plan_params("rate all 5 stars"))
    assert set(r) >= {"reply", "calls"} and r["reply"] == "Rating them 5 stars."
    assert r["calls"] == good["calls"] and r["repaired"] is False and r["warnings"] == []
    assert matches(r["calls"], [("set_rating", {"rating": 5})])
    # the grammar handed to the runtime is the tool-union schema, decoding budget bounded
    assert fake.calls[0]["schema"] == plan_schema(TOOL_LIST) and fake.calls[0]["max_tokens"] == 512


async def test_plan_repairs_once(mgr, hw) -> None:
    bad = {
        "calls": [{"tool": "set_rating", "args": {"selection": "current_filter", "rating": 9}}],
        "reply": "x",
    }
    good = {
        "calls": [{"tool": "set_rating", "args": {"selection": "current_filter", "rating": 4}}],
        "reply": "ok",
    }
    fake = FakeText(bad, good)
    r = await make(mgr, hw, text=fake).plan(plan_params("rate 4"))
    assert r["repaired"] is True and r["calls"][0]["args"]["rating"] == 4 and r["warnings"] == []
    assert len(fake.calls) == 2
    assert "not valid" in fake.calls[1]["messages"][-1]["content"]


async def test_plan_repairs_unparseable_text_and_prunes(mgr, hw) -> None:
    good = {
        "calls": [
            {
                "tool": "export",
                "args": {"selection": "current_filter", "preset": "wechat", "dest": "zzz"},
            }
        ],
        "reply": "ok",
    }
    fake = FakeText("I think you want to export!", "```json\n" + json.dumps(good) + "\n```")
    r = await make(mgr, hw, text=fake).plan(plan_params("export for wechat"))
    assert r["repaired"] and r["calls"][0]["args"] == {
        "selection": "current_filter",
        "preset": "wechat",
    }
    assert any("dropped invented" in w for w in r["warnings"])


async def test_plan_unsupported_has_empty_calls(mgr, hw) -> None:
    r = await make(
        mgr, hw, text=FakeText({"calls": [], "reply": "Sorry, I only handle photos."})
    ).plan(plan_params("book a flight"))
    assert r["calls"] == [] and r["reply"]


async def test_plan_never_returns_unknown_tools(mgr, hw) -> None:
    evil = {"calls": [{"tool": "rm_rf", "args": {"path": "/"}}], "reply": "ok"}
    r = await make(mgr, hw, text=FakeText(evil, evil)).plan(plan_params("do it"))
    assert r["calls"] == []


async def test_plan_unusable_output_is_an_error(mgr, hw) -> None:
    with pytest.raises(RpcError) as e:
        await make(mgr, hw, text=FakeText("nope", "still nope")).plan(plan_params("x"))
    assert e.value.code == llm_service.GENERATION_FAILED


async def test_plan_timeout_maps_to_error_code(mgr, hw) -> None:
    with pytest.raises(RpcError) as e:
        await make(mgr, hw, text=FakeText(GenerationTimeout("slow"))).plan(plan_params("x"))
    assert e.value.code == llm_service.GENERATION_TIMEOUT and e.value.kind == "timeout"


@pytest.mark.parametrize(
    "params",
    [
        {},
        {"message": ""},
        {"message": "x"},
        {"message": "x", "tools": []},
        {"message": "x", "tools": TOOLS, "context": "no"},
        {"message": "x", "tools": TOOLS, "max_tokens": "many"},
    ],
)
async def test_plan_invalid_params(mgr, hw, params) -> None:
    with pytest.raises(InvalidParams):
        await make(mgr, hw, text=FakeText()).plan(params)


@pytest.fixture
def photo(tmp_path: Path) -> dict[str, Any]:
    p = tmp_path / "p.jpg"
    img = np.dstack([np.tile(np.linspace(0, 255, 1600, dtype=np.uint8), (1000, 1))] * 3)
    cv2.imwrite(str(p), img)
    return {"photo_id": 7, "path": str(p)}


async def test_suggest_clamps_and_uses_resized_image(mgr, hw, tmp_path) -> None:
    p = tmp_path / "dark.jpg"
    cv2.imwrite(str(p), _solid(25, (1000, 1600)))
    out = {
        "problems": ["underexposed", "bogus", "noisy", "noisy"],
        "adjust": {"exposure": 40, "contrast": 500, "temp": -5000, "shadows": 20, "unknown": 1},
        "reason": " too dark ",
    }
    fake = FakeVision(out)
    r = await make(mgr, hw, vision=fake).suggest(
        {
            "photo": {"photo_id": 1, "path": str(p)},
            "context": {"scene_type": "night", "scores": {"iqa": 0.4}},
        }
    )
    assert max(fake.calls[0]["shape"][:2]) == 768
    assert "scene type: night" in fake.calls[0]["prompt"] and "iqa=0.4" in fake.calls[0]["prompt"]
    a = r["adjust"]
    assert 0 < a["exposure"] <= 5 and -100 <= a["contrast"] <= 100 and -3000 <= a["temp"] <= 3000
    assert a["exposure"] == baseline_adjust(image_facts(_solid(25)))["exposure"]  # measured wins
    assert "unknown" not in a and set(ADJUST_RANGES) <= set(a)
    assert r["problems"] == ["underexposed", "low_contrast", "noisy"]  # measured first, deduped
    assert r["reason"] == "too dark"
    assert r["locale"] == "en"


async def test_suggest_retries_unparseable_output(mgr, hw, photo) -> None:
    fake = FakeVision("sorry", {"problems": [], "adjust": {}, "reason": "fine"})
    r = await make(mgr, hw, vision=fake).suggest({"photo": photo})
    assert len(fake.calls) == 2 and r["reason"] == "fine"
    with pytest.raises(RpcError):
        await make(mgr, hw, vision=FakeVision("a", "b")).suggest({"photo": photo})


async def test_describe_en_and_translated(mgr, hw, photo) -> None:
    cap = {"caption": " A cat. ", "keywords": ["cat", "cat", "pet", " "]}
    r = await make(mgr, hw, vision=FakeVision(cap)).describe({"photo": photo, "locale": "en"})
    assert r["caption"] == "A cat." and r["keywords"] == ["cat", "pet"] and r["locale"] == "en"

    zh = {"caption": "一只猫。", "keywords": ["猫", "宠物"]}
    text = FakeText(zh)
    r = await make(mgr, hw, text=text, vision=FakeVision(cap)).describe(
        {"photo": photo, "locale": "zh-CN"}
    )
    assert r["caption"] == "一只猫。" and r["keywords"] == ["猫", "宠物"] and r["locale"] == "zh-CN"
    assert "Simplified Chinese" in text.calls[0]["messages"][0]["content"]
    assert "pattern" in text.calls[0]["schema"]["properties"]["caption"]


async def test_describe_falls_back_to_english_when_translation_fails(mgr, hw, photo) -> None:
    cap = {"caption": "A cat.", "keywords": ["cat", "pet", "fur"]}
    r = await make(mgr, hw, text=FakeText("garbage"), vision=FakeVision(cap)).describe(
        {"photo": photo, "locale": "zh-CN"}
    )
    assert r["caption"] == "A cat." and r["locale"] == "en"


async def test_suggest_translates_reason(mgr, hw, photo) -> None:
    out = {"problems": [], "adjust": {}, "reason": "Looks fine."}
    r = await make(
        mgr, hw, text=FakeText({"reason": "看起来不错。"}), vision=FakeVision(out)
    ).suggest({"photo": photo, "locale": "zh-CN"})
    assert r["reason"] == "看起来不错。" and r["locale"] == "zh-CN"


async def test_bad_photo_params(mgr, hw) -> None:
    with pytest.raises(InvalidParams):
        await make(mgr, hw, vision=FakeVision()).describe({"photo": {"path": "x"}})
    with pytest.raises(RpcError) as e:
        await make(mgr, hw, vision=FakeVision()).describe(
            {"photo": {"photo_id": 1, "path": "/nope.jpg"}}
        )
    assert e.value.code == errors.DECODE_FAILED


# ---------------------------------------------------------------------------- availability
async def test_unavailable_reports_models(mgr, hw, monkeypatch, photo) -> None:
    a = Assistant(mgr, hw)
    monkeypatch.setattr(llm_service, "genai_available", lambda: False)
    with pytest.raises(RpcError) as e:
        await a.plan(plan_params("x"))
    assert e.value.code == errors.MODEL_UNAVAILABLE
    d = e.value.to_obj()["data"]["detail"]
    assert d["models"] == ["phi-4-mini-genai-int4-cuda"] and d["reason"] == "runtime_missing"
    with pytest.raises(RpcError) as e:
        await a.suggest({"photo": photo})
    assert e.value.to_obj()["data"]["detail"]["models"] == ["phi-3.5-vision-genai-int4-cuda"]

    monkeypatch.setattr(llm_service, "genai_available", lambda: True)
    with pytest.raises(RpcError) as e:  # runtime present, model not installed
        await a.describe({"photo": photo})
    assert e.value.to_obj()["data"]["detail"]["reason"] == "not_installed"
    assert a.status()["llm_available"] is False and a.status()["vlm_available"] is False


def test_tier_gating_and_cpu_candidates(mgr, hw, monkeypatch) -> None:
    low = HardwareInfo("t", "c", 4, 8, 16000, tier="T1", device="cpu")
    m = ModelManager(mgr.models_dir, mgr.registry, low)
    a = Assistant(m, low)
    assert a.default_models() == {"llm": [], "vlm": []}  # T1 CPU: neither
    assert [s.id for s in a.candidates("llm")] == ["qwen3-4b-instruct-genai-int4"]  # no CUDA build
    assert [s.id for s in a.candidates("vlm")] == ["phi-3.5-vision-genai-int4-cpu"]
    a.any_tier = True
    assert a.default_models()["llm"] == ["qwen3-4b-instruct-genai-int4"]
    gpu1 = HardwareInfo("t", "c", 4, 8, 16000, tier="T1", device="cuda")
    assert Assistant(ModelManager(mgr.models_dir, mgr.registry, gpu1), gpu1).tier_ok("llm")
    assert not Assistant(ModelManager(mgr.models_dir, mgr.registry, gpu1), gpu1).tier_ok("vlm")
    assert Assistant(mgr, hw).default_models() == {
        "llm": ["phi-4-mini-genai-int4-cuda"],
        "vlm": ["phi-3.5-vision-genai-int4-cuda"],
    }


def test_registry_entries_are_permissive_optional_and_grouped() -> None:
    reg = Registry.load()
    for mid in [*llm_service.LLM_MODELS, *llm_service.VLM_MODELS]:
        s = reg.get(mid)
        assert s.license in ("MIT", "Apache-2.0") and s.optional and not s.noncommercial
        assert "assistant" in s.required_for and s.exclusive_group == "diffusion"
        assert all(f.sha256 and f.size for f in s.files)
        assert s.primary.path.endswith("genai_config.json")


# ---------------------------------------------------------------------------- worker wiring
async def test_service_info_list_delete(tmp_path, hw) -> None:
    svc = WorkerService(models_dir=str(tmp_path / "m"), hardware=hw, idle_unload_s=0)
    info = await svc.system_info({}, None)
    assert info["llm_available"] is False and info["vlm_available"] is False
    assert {"llm_available", "vlm_available"} <= set(info) and "assistant" in info
    listing = await svc.models_list({}, None)
    assert listing["assistant_models"] == {
        "llm": ["phi-4-mini-genai-int4-cuda"],
        "vlm": ["phi-3.5-vision-genai-int4-cuda"],
    }
    for name in ("llm.plan", "vlm.suggest", "vlm.describe", "models.delete"):
        assert name in svc._handlers()
    with pytest.raises(InvalidParams):
        await svc.models_delete({"id": "nope"}, None)
    with pytest.raises(InvalidParams):
        await svc.models_delete({}, None)
    d = svc.models_dir / "phi-4-mini-genai-int4-cuda"
    d.mkdir()
    (d / "x.bin").write_bytes(b"12345")
    assert await svc.models_delete({"id": "phi-4-mini-genai-int4-cuda"}, None) == {
        "deleted": True,
        "freed_bytes": 5,
    }
    assert not d.exists()
    await svc.stop()


async def test_unavailable_over_service_without_extra(tmp_path, hw, monkeypatch) -> None:
    monkeypatch.setattr(llm_service, "genai_available", lambda: False)
    svc = WorkerService(models_dir=str(tmp_path / "m"), hardware=hw, idle_unload_s=0)
    with pytest.raises(RpcError) as e:
        await svc.llm_plan(
            plan_params("x"), type("C", (), {"progress": staticmethod(lambda **k: None)})()
        )
    assert e.value.code == -32010
    await svc.stop()


def test_time_budget_is_bounded() -> None:
    assert llm_service._bounded({"timeout_s": 10**6}, "timeout_s", 60, 1, 600) == 600
    t = time.monotonic()
    assert llm_service._bounded({}, "timeout_s", 60, 1, 600) == 60 and time.monotonic() - t < 1
