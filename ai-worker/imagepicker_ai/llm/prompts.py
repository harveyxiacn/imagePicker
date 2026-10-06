"""Prompt construction for `llm.plan`, `vlm.suggest` and `vlm.describe` (docs/api-contract-m6.md A.3)."""

from __future__ import annotations

import json
from dataclasses import dataclass
from typing import Any

from .schema import Tool, validate

PROBLEM_TAGS = [
    "underexposed",
    "overexposed",
    "low_contrast",
    "blown_highlights",
    "crushed_shadows",
    "color_cast_warm",
    "color_cast_cool",
    "color_cast_green",
    "color_cast_magenta",
    "dull_colors",
    "oversaturated",
    "hazy",
    "noisy",
    "blurry",
    "tilted",
]


def language_name(locale: str | None) -> str:
    loc = (locale or "").lower()
    if loc.startswith("zh"):
        return "Simplified Chinese"
    if loc.startswith("ja"):
        return "Japanese"
    if loc.startswith("ko"):
        return "Korean"
    if loc.startswith(("de", "fr", "es", "it", "pt", "ru")):
        return {
            "de": "German",
            "fr": "French",
            "es": "Spanish",
            "it": "Italian",
            "pt": "Portuguese",
            "ru": "Russian",
        }[loc[:2]]
    return "English"


def is_zh(locale: str | None) -> bool:
    return (locale or "").lower().startswith("zh")


# ---------------------------------------------------------------------------- llm.plan
@dataclass(frozen=True)
class Example:
    locale: str  # "zh" | "en"
    user: str
    calls: list[dict[str, Any]]
    reply: str


_CF = "current_filter"
EXAMPLES: list[Example] = [
    Example(
        "zh",
        "把当前筛选出来的照片全部评为3星",
        [{"tool": "set_rating", "args": {"selection": _CF, "rating": 3}}],
        "好的，把当前筛选的照片评为3星。",
    ),
    Example(
        "zh",
        "把过曝的照片都淘汰掉",
        [
            {"tool": "filter", "args": {"issues_any": ["overexposed"]}},
            {"tool": "set_flag", "args": {"selection": _CF, "flag": -1}},
        ],
        "筛选出过曝的照片并标记为淘汰。",
    ),
    Example(
        "en",
        "auto-adjust all of them in landscape mode",
        [{"tool": "auto_adjust", "args": {"selection": _CF, "mode": "landscape"}}],
        "Running auto adjust in landscape mode.",
    ),
    Example(
        "en",
        "reject all the blurry photos",
        [
            {"tool": "filter", "args": {"issues_any": ["blurry"]}},
            {"tool": "set_flag", "args": {"selection": _CF, "flag": -1}},
        ],
        "Filtering the blurry photos and rejecting them.",
    ),
    Example(
        "zh",
        "每个连拍组只留最好的2张，其余淘汰",
        [{"tool": "group_keep_top", "args": {"selection": _CF, "n": 2, "reject_rest": True}}],
        "每个连拍组保留前2张，其余标为淘汰。",
    ),
    Example(
        "en",
        "export these for Instagram",
        [{"tool": "export", "args": {"selection": _CF, "preset": "instagram"}}],
        "Exporting the current photos with the Instagram preset.",
    ),
    Example(
        "zh",
        "找出4星以上而且有人闭眼的照片",
        [{"tool": "filter", "args": {"rating_gte": 4, "issues_any": ["closed_eyes"]}}],
        "已筛选4星以上且有人闭眼的照片。",
    ),
    Example(
        "en",
        "pick the best 3 of every scene and clear the rest",
        [{"tool": "scene_keep_top", "args": {"selection": _CF, "n": 3, "reject_rest": True}}],
        "Keeping the top 3 per scene and rejecting the others.",
    ),
    Example(
        "zh",
        "帮我订一张去京都的机票",
        [],
        "抱歉，我只能筛选、评分、修图和导出照片，比如：“把4星以上的照片导出为小红书尺寸”。",
    ),
]


def usable_examples(tools: list[Tool]) -> list[Example]:
    """Examples whose every call validates against the supplied tool schemas."""
    by = {t.name: t for t in tools}
    out = []
    for ex in EXAMPLES:
        ok = True
        for c in ex.calls:
            t = by.get(c["tool"])
            if t is None or validate(t.schema, c["args"]):
                ok = False
                break
        if ok:
            out.append(ex)
    return out


def context_examples(tools: list[Tool], context: dict[str, Any] | None) -> list[Example]:
    """Examples tied to the caller's context: "selected" -> its ids, "this photo" -> its photo id."""
    ctx = context if isinstance(context, dict) else {}
    cand: list[Example] = []
    ids = ctx.get("selection")
    if isinstance(ids, list) and ids and all(isinstance(i, int) for i in ids):
        sel = {"ids": ids[:20]}
        cand += [
            Example(
                "en",
                "set the selected photos to 2 stars",
                [{"tool": "set_rating", "args": {"selection": sel, "rating": 2}}],
                "Setting the selected photos to 2 stars.",
            ),
            Example(
                "zh",
                "把选中的照片标记为精选",
                [{"tool": "set_flag", "args": {"selection": sel, "flag": 1}}],
                "把选中的照片标记为精选。",
            ),
        ]
    cur = ctx.get("current_photo_id")
    if isinstance(cur, int) and not isinstance(cur, bool):
        cand += [
            Example(
                "en",
                "suggest some edits for this photo",
                [{"tool": "suggest_edits", "args": {"photo_id": cur}}],
                "Asking for edit suggestions for the current photo.",
            ),
            Example(
                "zh",
                "为这张照片生成关键词",
                [{"tool": "describe", "args": {"photo_id": cur}}],
                "为当前这张照片生成描述和关键词。",
            ),
        ]
    by = {t.name: t for t in tools}
    return [
        e
        for e in cand
        if all(by.get(c["tool"]) and not validate(by[c["tool"]].schema, c["args"]) for c in e.calls)
    ]


def _compact(obj: Any) -> str:
    return json.dumps(obj, ensure_ascii=False, separators=(",", ":"))


def _type_str(sc: Any, depth: int = 0) -> str:
    """Compact TypeScript-like rendering of a JSON schema (the grammar enforces the exact shape)."""
    if not isinstance(sc, dict):
        return "any"
    if "const" in sc:
        return _compact(sc["const"])
    if "enum" in sc:
        return "|".join(_compact(v) for v in sc["enum"])
    for key in ("anyOf", "oneOf"):
        if key in sc:
            return "|".join(_type_str(b, depth + 1) for b in sc[key])
    t = sc.get("type")
    if isinstance(t, list):
        return "|".join("null" if x == "null" else _type_str({**sc, "type": x}, depth) for x in t)
    if t == "object":
        props = sc.get("properties") or {}
        if not props:
            return "object"
        req = set(sc.get("required") or [])
        parts = [
            f"{k}{'' if k in req else '?'}: {_type_str(v, depth + 1)}" for k, v in props.items()
        ]
        return "{" + ", ".join(parts) + "}"
    if t == "array":
        return "[" + _type_str(sc.get("items"), depth + 1) + "]"
    if t in ("integer", "number"):
        lo, hi = sc.get("minimum"), sc.get("maximum")
        rng = f" {lo}..{hi}" if lo is not None and hi is not None else ""
        return ("int" if t == "integer" else "number") + rng
    if t == "boolean":
        return "bool"
    if t == "string":
        return "string"
    return "any"


def _tool_line(t: Tool) -> str:
    desc = " ".join(t.description.split())[:160]
    return f"- {t.name}({_type_str(t.schema)[1:-1]}): {desc}"


def _context_lines(context: dict[str, Any] | None) -> list[str]:
    ctx = context if isinstance(context, dict) else {}
    lines: list[str] = []
    flt = ctx.get("filter")
    lines.append(
        f'- current view / filter ("these", "all"): {flt!r}'
        if flt
        else '- current view ("these", "all"): all photos, no filter'
    )
    sel = ctx.get("selection")
    if isinstance(sel, list) and sel:
        shown = ", ".join(str(i) for i in sel[:20])
        more = f" (+{len(sel) - 20} more)" if len(sel) > 20 else ""
        lines.append(f'- selected photo ids ("the selected photos", {len(sel)}): [{shown}]{more}')
    else:
        lines.append('- selected photo ids ("the selected photos"): none')
    cur = ctx.get("current_photo_id")
    lines.append(
        f'- current photo id ("this photo"): {cur}'
        if cur is not None
        else '- current photo id ("this photo"): none'
    )
    for key, label in (
        ("presets", "available presets"),
        ("people", "known people"),
        ("collections", "collections"),
    ):
        v = ctx.get(key)
        if isinstance(v, list) and v:
            lines.append(f"- {label}: {_compact(v[:40])}")
    return lines


def build_system_prompt(
    tools: list[Tool], context: dict[str, Any] | None, locale: str | None
) -> str:
    lang = language_name(locale)
    head = (
        "You are the planning module of imagePicker, a local photo culling and editing app. "
        "Turn the user's request into a short list of tool calls. You only PLAN: nothing runs until "
        "the user confirms, so never claim that something was already done.\n"
        "\n"
        "Output exactly one JSON object and nothing else:\n"
        '{"calls": [{"tool": "<tool name>", "args": {...}}], "reply": "<one short sentence>"}\n'
        "\n"
        "Rules:\n"
        "- Use only the tools listed below and only their arguments (`name?` = optional). Never invent "
        "tool names, argument names, values or photo ids.\n"
        "- Use as few calls as possible, in execution order. A later call that says "
        f'"selection": "{_CF}" works on the photos produced by the earlier `filter` call (or the '
        "current view when there is none).\n"
        f'- `selection` is "{_CF}" (every photo in the current view), {{"ids": [...]}} (only ids '
        'given in the context) or {"query": "..."}. "these", "all", "the filtered photos" mean the '
        'current view; "the selected photos" means {"ids": <selected photo ids>} (when none are '
        'selected use the current view); "this photo" means the current photo id.\n'
        "- Copy every option the request states (counts, stars, presets, modes, sizes, platforms) into the arguments. Leave out optional arguments it does not mention; never make up values.\n"
        '- To act on photos that match a condition ("reject all blurry photos"), first call `filter` with the condition, then the action tool on the current view.\n'
        "- Chinese terms: 淘汰/拒绝 = reject (flag -1); 选用/留用/标记为精选 = pick (flag 1); 模糊 = "
        "blurry; 闭眼 = closed eyes; 欠曝/过曝 = under/overexposed; 夜景 = night scene; 连拍组 = "
        "burst group; 场景 = scene; 小红书 = xiaohongshu; 微信/朋友圈 = wechat; 筛选/找出/显示 = filter; "
        "导出 = export; AI评分 = AI rating; 一键合成最佳表情 = Best Take.\n"
        "- Ratings are 0-5 stars (null clears). Flags: 1 = pick, -1 = reject, 0 = clear.\n"
        "- If the request cannot be done with these tools, or is not about photos, return an empty "
        '"calls" array and use "reply" to say so briefly with one example of something you can do.\n'
        f'- The user may write Chinese or English. Write "reply" in the language of the request '
        f"(default: {lang}), one sentence, no markdown.\n"
        "\n"
        "Tools:\n" + "\n".join(_tool_line(t) for t in tools) + "\n\n"
        "Current state:\n" + "\n".join(_context_lines(context))
    )
    return head


def build_plan_messages(
    message: str, tools: list[Tool], context: dict[str, Any] | None, locale: str | None
) -> list[dict[str, str]]:
    msgs = [{"role": "system", "content": build_system_prompt(tools, context, locale)}]
    exs = usable_examples(tools) + context_examples(tools, context)
    want = "zh" if is_zh(locale) else "en"
    exs = [e for e in exs if e.locale != want] + [e for e in exs if e.locale == want]
    for e in exs[-8:]:
        msgs.append({"role": "user", "content": e.user})
        msgs.append(
            {"role": "assistant", "content": _compact({"calls": e.calls, "reply": e.reply})}
        )
    msgs.append({"role": "user", "content": message})
    return msgs


def repair_messages(
    messages: list[dict[str, str]], output: str, problems: list[str]
) -> list[dict[str, str]]:
    shown = "; ".join(problems[:6])
    return [
        *messages,
        {"role": "assistant", "content": output.strip()[:2000]},
        {
            "role": "user",
            "content": "That reply was not valid: " + shown + ". Answer the previous request again "
            'with one valid JSON object {"calls": [...], "reply": "..."} that follows the tool schemas.',
        },
    ]


# ---------------------------------------------------------------------------- vision
SUGGEST_RULES = (
    "You are an expert photo editor. Look at the photo and the measurements, then suggest global "
    "Lightroom-style slider changes that make it look better while keeping the photographer's "
    "intent.\n"
    "Sliders (0 = unchanged): exposure in EV stops (-3 to 3, positive brightens); contrast, "
    "highlights (negative recovers bright areas), shadows (positive lifts dark areas), whites, "
    "blacks, tint (negative green, positive magenta), vibrance, saturation, clarity, dehaze are "
    "integers from -100 to 100; temp is a Kelvin shift (-1500 to 1500, positive = warmer).\n"
    "Guidance: change only what the picture needs: most photos need 2-4 sliders, every other "
    "slider must be exactly 0 (never fill sliders with small default values). Mean luma below 0.35 "
    "means underexposed: raise exposure (about +0.5 EV per 0.07 below 0.45) and shadows. Above 0.65 "
    "means overexposed: lower exposure and highlights. Clipped highlights over 2%: lower highlights "
    "and whites. Crushed shadows over 5%: raise shadows and blacks. A strong red/blue imbalance "
    "means a colour cast: counter it with temp. Flat, grey-looking pictures need contrast, "
    "clarity or dehaze; a good picture needs little or nothing.\n"
    '"problems" lists what is wrong, chosen only from: ' + ", ".join(PROBLEM_TAGS) + " (at most 5, "
    "empty when the photo is fine).\n"
)


def build_suggest_prompt(
    measurements: str, locale: str | None, baseline: dict[str, Any] | None = None
) -> str:
    start = ""
    if baseline is not None:
        start = (
            "Starting point computed from the measurements (sliders not listed are 0): "
            + (_compact(baseline) if baseline else "{} (the photo needs no change)")
            + ". Keep it unless the picture clearly calls for something different.\n\n"
        )
    return (
        SUGGEST_RULES
        + f'"reason" is one or two sentences in {language_name(locale)} explaining the edit.\n\n'
        "Measurements of this photo:\n"
        + measurements
        + "\n\n"
        + start
        + 'Reply with one JSON object only: {"problems": [...], "adjust": {"exposure": ..., '
        '"contrast": ..., "highlights": ..., "shadows": ..., "whites": ..., "blacks": ..., '
        '"temp": ..., "tint": ..., "vibrance": ..., "saturation": ..., "clarity": ..., '
        '"dehaze": ...}, "reason": "..."}'
    )


def build_describe_prompt() -> str:
    return (
        "Describe this photo in one sentence (subject, setting, light or mood) and give 3 to 8 short "
        "keywords, all in English. "
        'Reply with one JSON object only: {"caption": string, "keywords": [string]}'
    )


def suggest_schema(adjust: dict[str, Any]) -> dict[str, Any]:
    return {
        "type": "object",
        "properties": {
            "problems": {
                "type": "array",
                "items": {"enum": PROBLEM_TAGS},
                "maxItems": 5,
            },
            "adjust": adjust,
            "reason": {"type": "string", "maxLength": 300},
        },
        "required": ["problems", "adjust", "reason"],
        "additionalProperties": False,
    }


def describe_schema(locale: str | None = "en") -> dict[str, Any]:
    return {
        "type": "object",
        "properties": {
            "caption": {"type": "string", "maxLength": 240},
            "keywords": {
                "type": "array",
                "items": {"type": "string", "maxLength": 32},
                "minItems": 3,
                "maxItems": 8,
            },
        },
        "required": ["caption", "keywords"],
        "additionalProperties": False,
    }


# ---------------------------------------------------------------------------- localisation
# Phi-3.5-vision writes poor Chinese (and ignores other target languages), so the vision model
# always answers in English and the text LLM translates the result when another locale is wanted.
def needs_translation(locale: str | None) -> bool:
    return bool(locale) and not (locale or "").lower().startswith("en")


def _zh_text_pattern(limit: int) -> str:
    # must start with a CJK character (keeps the English original from slipping through the grammar)
    return (
        "^[\u4e00-\u9fff][\u4e00-\u9fff\uff0c\u3002\u3001\uff1b\uff1a\uff01\uff1f\u201c\u201d\uff08\uff09"
        + (f"0-9A-Za-z \\-]{{3,{limit - 1}}}$")
    )


ZH_KEYWORD_PATTERN = "^[\u4e00-\u9fff][\u4e00-\u9fff0-9A-Za-z]{0,9}$"


def translate_schema(payload: dict[str, Any], locale: str | None) -> dict[str, Any]:
    """Grammar for the translated copy of `payload` (same keys, strings or lists of strings)."""
    zh = is_zh(locale)
    props: dict[str, Any] = {}
    for k, v in payload.items():
        if isinstance(v, list):
            item: dict[str, Any] = {"type": "string", "maxLength": 32}
            if zh:
                item = {"type": "string", "pattern": ZH_KEYWORD_PATTERN}
            props[k] = {
                "type": "array",
                "items": item,
                "minItems": min(len(v), 3),
                "maxItems": max(len(v), 1),
            }
        else:
            props[k] = (
                {"type": "string", "pattern": _zh_text_pattern(300)}
                if zh
                else {"type": "string", "maxLength": 300}
            )
    return {
        "type": "object",
        "properties": props,
        "required": list(props),
        "additionalProperties": False,
    }


def build_translate_messages(payload: dict[str, Any], locale: str | None) -> list[dict[str, str]]:
    return [
        {
            "role": "system",
            "content": "You translate short texts of a photo app into "
            f"{language_name(locale)}. The user sends a JSON object; translate every string value, "
            "keep the keys and the number of list items, write natural idiomatic wording and "
            "translate proper nouns the usual way. Reply with the translated JSON object only.",
        },
        {"role": "user", "content": _compact(payload)},
    ]
