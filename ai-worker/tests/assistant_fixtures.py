"""Tool schemas (docs/api-contract-m6.md A.1) + the 15 assistant commands used by the real-model
tests and `scripts/bench_assistant.py`."""

from __future__ import annotations

from typing import Any

_SEL: dict[str, Any] = {
    "description": 'Photos to act on: {"ids": [...]}, {"query": "<photos query string>"} or '
    '"current_filter" (everything in the current view).',
    "anyOf": [
        {
            "type": "object",
            "properties": {"ids": {"type": "array", "items": {"type": "integer"}, "minItems": 1}},
            "required": ["ids"],
        },
        {
            "type": "object",
            "properties": {"query": {"type": "string"}},
            "required": ["query"],
        },
        {"enum": ["current_filter"]},
    ],
}
ISSUES = ["closed_eyes", "blurry", "overexposed", "underexposed", "noisy", "tilted"]
SCENES = ["portrait", "group", "landscape", "food", "architecture", "night", "pet", "other"]


def _tool(name: str, desc: str, props: dict[str, Any], required: list[str]) -> dict[str, Any]:
    return {
        "name": name,
        "description": desc,
        "parameters": {"type": "object", "properties": props, "required": required},
    }


TOOLS: list[dict[str, Any]] = [
    _tool(
        "filter",
        "Change the UI filter only (no data is modified). Same fields as the /api/photos query.",
        {
            "rating_gte": {"type": "integer", "minimum": 0, "maximum": 5},
            "ai_rating_gte": {"type": "integer", "minimum": 0, "maximum": 5},
            "flag": {"enum": ["picked", "rejected", "unflagged", "not_rejected"]},
            "color_label": {"type": "string"},
            "issues_any": {"type": "array", "items": {"enum": ISSUES}},
            "issues_none": {"type": "boolean"},
            "scene_type": {"enum": SCENES},
            "persons": {"type": "array", "items": {"type": "string"}},
            "burst_best_only": {"type": "boolean"},
            "collection": {"type": "string"},
        },
        [],
    ),
    _tool(
        "set_rating",
        "Set the star rating (0-5, null clears).",
        {"selection": _SEL, "rating": {"type": ["integer", "null"], "minimum": 0, "maximum": 5}},
        ["selection", "rating"],
    ),
    _tool(
        "set_flag",
        "Set the flag: 1 pick, -1 reject, 0 none.",
        {"selection": _SEL, "flag": {"enum": [-1, 0, 1]}},
        ["selection", "flag"],
    ),
    _tool(
        "accept_ai",
        "Accept the AI star rating as the user rating.",
        {"selection": _SEL},
        ["selection"],
    ),
    _tool(
        "group_keep_top",
        "In every burst group keep the best n photos; optionally reject the rest.",
        {
            "selection": _SEL,
            "n": {"type": "integer", "minimum": 1, "maximum": 20},
            "reject_rest": {"type": "boolean"},
        },
        ["n", "reject_rest"],
    ),
    _tool(
        "scene_keep_top",
        "In every scene keep the best n photos; optionally reject the rest.",
        {
            "selection": _SEL,
            "n": {"type": "integer", "minimum": 1, "maximum": 20},
            "reject_rest": {"type": "boolean"},
        },
        ["n", "reject_rest"],
    ),
    _tool(
        "apply_preset",
        "Apply an edit preset.",
        {"selection": _SEL, "preset_id": {"type": "string"}},
        ["selection", "preset_id"],
    ),
    _tool(
        "auto_adjust",
        "One-click AI adjustment of every photo, saved per photo.",
        {"selection": _SEL, "mode": {"enum": ["auto", "portrait", "landscape"]}},
        ["selection", "mode"],
    ),
    _tool(
        "apply_profiles", "Apply the people's beauty profiles.", {"selection": _SEL}, ["selection"]
    ),
    _tool(
        "besttake_auto",
        "Best Take: composite everybody's best face for the selected burst groups.",
        {"selection": _SEL},
        ["selection"],
    ),
    _tool(
        "remove_bystanders",
        "Remove bystanders from the photos.",
        {"selection": _SEL},
        ["selection"],
    ),
    _tool(
        "export",
        "Export photos with a preset (the UI asks for the folder when dest is missing).",
        {
            "selection": _SEL,
            "preset": {"enum": ["original", "wechat", "xiaohongshu", "instagram"]},
            "dest": {"type": "string"},
        },
        ["selection"],
    ),
    _tool(
        "describe",
        "Write a caption + keywords for one photo (VLM).",
        {"photo_id": {"type": "integer"}},
        ["photo_id"],
    ),
    _tool(
        "suggest_edits",
        "Suggest edit sliders for one photo (VLM, not saved).",
        {"photo_id": {"type": "integer"}},
        ["photo_id"],
    ),
]

CONTEXT: dict[str, Any] = {
    "filter": "scene_type=portrait",
    "selection": [101, 102, 103],
    "current_photo_id": 42,
}

# (message, locale, expected calls). An expected arg value matches when equal, or (for a list)
# when it contains every expected element; calls to tools outside the expectation fail the case.
COMMANDS: list[tuple[str, str, list[tuple[str, dict[str, Any]]]]] = [
    ("把当前筛选的照片全部评为5星", "zh-CN", [("set_rating", {"rating": 5})]),
    ("Rate the selected photos 3 stars", "en", [("set_rating", {"rating": 3})]),
    (
        "淘汰所有模糊的照片",
        "zh-CN",
        [("filter", {"issues_any": ["blurry"]}), ("set_flag", {"flag": -1})],
    ),
    (
        "Show me photos rated 4 stars or above that have closed eyes",
        "en",
        [("filter", {"rating_gte": 4, "issues_any": ["closed_eyes"]})],
    ),
    (
        "每个连拍组只保留最好的3张，其余淘汰",
        "zh-CN",
        [("group_keep_top", {"n": 3, "reject_rest": True})],
    ),
    (
        "Keep the top 2 of each scene and reject the rest",
        "en",
        [("scene_keep_top", {"n": 2, "reject_rest": True})],
    ),
    ("把这些照片导出为小红书尺寸", "zh-CN", [("export", {"preset": "xiaohongshu"})]),
    ("Export everything for WeChat", "en", [("export", {"preset": "wechat"})]),
    ("接受AI评分", "zh-CN", [("accept_ai", {})]),
    (
        "Run one-click auto adjust on the selected photos in portrait mode",
        "en",
        [("auto_adjust", {"mode": "portrait"})],
    ),
    ("把夜景照片筛选出来", "zh-CN", [("filter", {"scene_type": "night"})]),
    ("Flag the selected photos as picks", "en", [("set_flag", {"flag": 1})]),
    ("对选中的连拍组一键合成最佳表情", "zh-CN", [("besttake_auto", {})]),
    ("Apply the beauty profiles to the selected photos", "en", [("apply_profiles", {})]),
    ("给这张照片写个描述", "zh-CN", [("describe", {"photo_id": 42})]),
]


def _arg_ok(want: Any, got: Any) -> bool:
    if isinstance(want, list):
        return isinstance(got, list) and all(w in got for w in want)
    return bool(want == got and type(want) is type(got))


def matches(calls: list[dict[str, Any]], expected: list[tuple[str, dict[str, Any]]]) -> bool:
    """Every expected call is present (args subset) and nothing outside the expected tools is."""
    names = {n for n, _ in expected}
    if any(c["tool"] not in names for c in calls):
        return False
    for name, args in expected:
        if not any(
            c["tool"] == name
            and all(k in c["args"] and _arg_ok(v, c["args"][k]) for k, v in args.items())
            for c in calls
        ):
            return False
    return True
