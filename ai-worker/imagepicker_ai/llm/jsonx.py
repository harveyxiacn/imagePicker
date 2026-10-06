"""Robust JSON extraction / repair for small-model output.

With grammar-constrained decoding the model already emits valid JSON; this module is the safety
net for the cases where the constraint is unavailable (old runtime, unsupported schema keyword) or
the output was cut by the token limit: markdown fences, `<think>` blocks, leading chatter, trailing
commas, smart quotes, Python literals and truncated structures are handled.
"""

from __future__ import annotations

import json
import re
from typing import Any


class JsonExtractError(ValueError):
    pass


_THINK = re.compile(r"<think>.*?(?:</think>|$)", re.S | re.I)
_FENCE = re.compile(r"```(?:json|JSON)?\s*(.*?)(?:```|$)", re.S)
_TRAILING_COMMA = re.compile(r",\s*([}\]])")
_SMART = str.maketrans({"\u201c": '"', "\u201d": '"', "\u2018": "'", "\u2019": "'"})
_PY_LITERALS = [
    (re.compile(r"\bTrue\b"), "true"),
    (re.compile(r"\bFalse\b"), "false"),
    (re.compile(r"\bNone\b"), "null"),
]


def strip_wrappers(text: str) -> str:
    """Remove `<think>` blocks and markdown fences."""
    text = _THINK.sub("", text).strip()
    m = _FENCE.search(text)
    if m and m.group(1).strip():
        text = m.group(1).strip()
    return text


def _scan_structure(s: str) -> tuple[list[str], bool, int]:
    """(open bracket stack, inside_string, index after the last complete top-level value)."""
    stack: list[str] = []
    in_str = False
    esc = False
    end = -1
    for i, ch in enumerate(s):
        if in_str:
            if esc:
                esc = False
            elif ch == "\\":
                esc = True
            elif ch == '"':
                in_str = False
            continue
        if ch == '"':
            in_str = True
        elif ch in "{[":
            stack.append(ch)
        elif ch in "}]" and stack:
            stack.pop()
            if not stack:
                end = i + 1
    return stack, in_str, end


def close_truncated(s: str) -> str:
    """Best effort to close a structure cut off mid-way (token limit)."""
    stack, in_str, _ = _scan_structure(s)
    if not stack:
        return s
    out = s
    if in_str:
        if out.endswith("\\"):
            out = out[:-1]
        out += '"'
    # drop a dangling `"key":` / `,` / `:`
    out = out.rstrip()
    out = re.sub(r',\s*"[^"\\]*"\s*:\s*$', "", out)
    out = re.sub(r'\{\s*"[^"\\]*"\s*:\s*$', "{", out)
    out = re.sub(r"[,:]\s*$", "", out)
    stack, _, _ = _scan_structure(out)
    return out + "".join("}" if b == "{" else "]" for b in reversed(stack))


def _fix_syntax(s: str) -> str:
    s = s.translate(_SMART)
    s = _TRAILING_COMMA.sub(r"\1", s)
    for pat, rep in _PY_LITERALS:
        s = pat.sub(rep, s)
    if '"' not in s and "'" in s:  # single-quoted pseudo-JSON
        s = s.replace("'", '"')
    return s


def _candidates(text: str) -> list[str]:
    out = []
    for m in re.finditer(r"[{\[]", text):
        out.append(text[m.start() :])
    return out


def extract_json(text: str, expect: type | tuple[type, ...] = dict) -> Any:
    """First JSON value of type `expect` found in `text` (repairing it if necessary).

    Raises JsonExtractError when nothing usable is found.
    """
    if not isinstance(text, str) or not text.strip():
        raise JsonExtractError("empty output")
    text = strip_wrappers(text)
    dec = json.JSONDecoder()
    first_error: str | None = None
    for cand in _candidates(text):
        for fixer in (lambda x: x, _fix_syntax, lambda x: close_truncated(_fix_syntax(x))):
            try:
                val, _ = dec.raw_decode(fixer(cand))
            except json.JSONDecodeError as e:
                first_error = first_error or str(e)
                continue
            if isinstance(val, expect):
                return val
            break
    raise JsonExtractError(first_error or "no JSON object found in the output")
