"""Tool schemas: normalisation, a small JSON-Schema validator, and the grammar schema of a plan.

The validator covers what tool argument schemas use (type, enum, const, required, properties,
additionalProperties, items, min/max, length, anyOf/oneOf/allOf, local $ref); it is not a full
implementation. Core re-validates every call before anything runs, this is the first line of defence
(and drives the one repair retry).
"""

from __future__ import annotations

import copy
import json
import re
from dataclasses import dataclass
from typing import Any

from ..errors import InvalidParams

MAX_TOOLS = 40
MAX_CALLS = 8
_NAME = re.compile(r"^[A-Za-z_][A-Za-z0-9_.\-]{0,63}$")
# annotation / unsupported keywords dropped before the schema is handed to the grammar engine
_STRIP = {
    "description",
    "title",
    "examples",
    "example",
    "default",
    "$schema",
    "$comment",
    "format",
    "deprecated",
    "readOnly",
    "writeOnly",
    "$id",
}


@dataclass(frozen=True)
class Tool:
    name: str
    description: str
    schema: dict[str, Any]  # JSON schema of the `args` object


def _tool(obj: Any, idx: int) -> Tool:
    if not isinstance(obj, dict):
        raise InvalidParams(f"tools[{idx}] must be an object")
    fn = obj.get("function") if isinstance(obj.get("function"), dict) else obj
    name = fn.get("name") or fn.get("tool") or fn.get("title")
    if not isinstance(name, str) or not _NAME.match(name):
        raise InvalidParams(f"tools[{idx}] needs a valid `name`")
    schema = None
    for key in ("parameters", "input_schema", "args", "schema", "arguments"):
        if isinstance(fn.get(key), dict):
            schema = fn[key]
            break
    if schema is None and fn.get("type") == "object" and "properties" in fn:
        schema = {k: v for k, v in fn.items() if k not in ("name", "tool", "title", "description")}
    if schema is None:
        schema = {"type": "object", "properties": {}}
    desc = fn.get("description") or ""
    return Tool(name, str(desc), schema)


def parse_tools(raw: Any) -> list[Tool]:
    if not isinstance(raw, list) or not raw:
        raise InvalidParams("tools must be a non-empty array of tool JSON schemas")
    if len(raw) > MAX_TOOLS:
        raise InvalidParams(f"at most {MAX_TOOLS} tools")
    tools = [_tool(t, i) for i, t in enumerate(raw)]
    names = [t.name for t in tools]
    if len(set(names)) != len(names):
        raise InvalidParams("duplicate tool names")
    return tools


# ---------------------------------------------------------------------------- validator
def _resolve(ref: str, root: dict[str, Any]) -> dict[str, Any]:
    if not ref.startswith("#/"):
        return {}
    node: Any = root
    for part in ref[2:].split("/"):
        node = (
            node.get(part.replace("~1", "/").replace("~0", "~")) if isinstance(node, dict) else None
        )
    return node if isinstance(node, dict) else {}


def _is_type(v: Any, t: str) -> bool:
    if t == "object":
        return isinstance(v, dict)
    if t == "array":
        return isinstance(v, list)
    if t == "string":
        return isinstance(v, str)
    if t == "boolean":
        return isinstance(v, bool)
    if t == "null":
        return v is None
    if t == "integer":
        return (isinstance(v, int) and not isinstance(v, bool)) or (
            isinstance(v, float) and v.is_integer()
        )
    if t == "number":
        return isinstance(v, (int, float)) and not isinstance(v, bool)
    return True


def validate(
    schema: dict[str, Any], value: Any, path: str = "$", root: dict[str, Any] | None = None
) -> list[str]:
    """List of human-readable violations ([] = valid)."""
    root = root if root is not None else schema
    if not isinstance(schema, dict):
        return []
    if "$ref" in schema:
        return validate(_resolve(schema["$ref"], root), value, path, root)
    errs: list[str] = []
    if "const" in schema and value != schema["const"]:
        return [f"{path}: must be {schema['const']!r}"]
    if "enum" in schema and value not in schema["enum"]:
        return [f"{path}: must be one of {schema['enum']!r}"]
    t = schema.get("type")
    if t is not None:
        types = t if isinstance(t, list) else [t]
        if not any(_is_type(value, x) for x in types):
            return [f"{path}: expected {'|'.join(types)}, got {type(value).__name__}"]
    for key in ("anyOf", "oneOf"):
        if key in schema:
            branches = [validate(b, value, path, root) for b in schema[key]]
            if branches and all(branches):
                best = min(branches, key=len)
                return [f"{path}: matches none of the allowed shapes ({best[0]})"]
    for b in schema.get("allOf", []):
        errs += validate(b, value, path, root)
    if isinstance(value, dict):
        props = schema.get("properties", {})
        for r in schema.get("required", []):
            if r not in value:
                errs.append(f"{path}: missing required field {r!r}")
        for k, v in value.items():
            if k in props:
                errs += validate(props[k], v, f"{path}.{k}", root)
            elif schema.get("additionalProperties") is False:
                errs.append(f"{path}: unknown field {k!r}")
            elif isinstance(schema.get("additionalProperties"), dict):
                errs += validate(schema["additionalProperties"], v, f"{path}.{k}", root)
    elif isinstance(value, list):
        if "minItems" in schema and len(value) < schema["minItems"]:
            errs.append(f"{path}: needs at least {schema['minItems']} items")
        if "maxItems" in schema and len(value) > schema["maxItems"]:
            errs.append(f"{path}: at most {schema['maxItems']} items")
        if isinstance(schema.get("items"), dict):
            for i, v in enumerate(value):
                errs += validate(schema["items"], v, f"{path}[{i}]", root)
    elif isinstance(value, str):
        if "minLength" in schema and len(value) < schema["minLength"]:
            errs.append(f"{path}: too short")
        if "maxLength" in schema and len(value) > schema["maxLength"]:
            errs.append(f"{path}: too long")
        if "pattern" in schema and not re.search(schema["pattern"], value):
            errs.append(f"{path}: does not match {schema['pattern']!r}")
    elif isinstance(value, (int, float)) and not isinstance(value, bool):
        if "minimum" in schema and value < schema["minimum"]:
            errs.append(f"{path}: must be >= {schema['minimum']}")
        if "maximum" in schema and value > schema["maximum"]:
            errs.append(f"{path}: must be <= {schema['maximum']}")
    return errs


# ---------------------------------------------------------------------------- grammar schema
def grammar_ready(schema: Any) -> Any:
    """Copy of a tool schema without annotations; objects with `properties` are closed."""
    if isinstance(schema, list):
        return [grammar_ready(x) for x in schema]
    if not isinstance(schema, dict):
        return schema
    out = {k: grammar_ready(v) for k, v in schema.items() if k not in _STRIP}
    if "properties" in out and isinstance(out["properties"], dict):
        # properties maps names -> schemas; names must survive even when called "format" etc.
        out["properties"] = {k: grammar_ready(v) for k, v in schema["properties"].items()}
        out.setdefault("additionalProperties", False)
    return out


def plan_schema(tools: list[Tool], max_reply_chars: int = 240) -> dict[str, Any]:
    """Grammar for `{"calls": [{"tool", "args"}...], "reply": str}`: one anyOf branch per tool."""
    branches = []
    for t in tools:
        args = grammar_ready(copy.deepcopy(t.schema))
        args.setdefault("type", "object")
        branches.append(
            {
                "type": "object",
                "properties": {"tool": {"const": t.name}, "args": args},
                "required": ["tool", "args"],
                "additionalProperties": False,
            }
        )
    item: dict[str, Any] = branches[0] if len(branches) == 1 else {"anyOf": branches}
    return {
        "type": "object",
        "properties": {
            "calls": {"type": "array", "items": item, "maxItems": MAX_CALLS},
            "reply": {"type": "string", "maxLength": max_reply_chars},
        },
        "required": ["calls", "reply"],
        "additionalProperties": False,
    }


def check_plan(plan: Any, tools: list[Tool]) -> tuple[str, list[dict[str, Any]], list[str]]:
    """Validate a parsed plan against the tools -> (reply, valid calls, problems)."""
    by_name = {t.name: t for t in tools}
    if not isinstance(plan, dict):
        return "", [], ["output is not a JSON object"]
    problems: list[str] = []
    reply = plan.get("reply", "")
    if not isinstance(reply, str):
        problems.append("`reply` must be a string")
        reply = ""
    calls_raw = plan.get("calls", [])
    if not isinstance(calls_raw, list):
        problems.append("`calls` must be an array")
        calls_raw = []
    calls: list[dict[str, Any]] = []
    for i, c in enumerate(calls_raw[:MAX_CALLS]):
        if not isinstance(c, dict):
            problems.append(f"calls[{i}] is not an object")
            continue
        name = c.get("tool")
        tool = by_name.get(name) if isinstance(name, str) else None
        if tool is None:
            problems.append(f"calls[{i}]: unknown tool {name!r}")
            continue
        args = c.get("args", {})
        errs = validate(tool.schema, args, f"calls[{i}].args")
        if errs:
            problems += errs
            continue
        calls.append({"tool": tool.name, "args": args})
    if len(calls_raw) > MAX_CALLS:
        problems.append(f"too many calls (max {MAX_CALLS})")
    return reply.strip(), calls, problems


def prune_invented(
    calls: list[dict[str, Any]], tools: list[Tool], message: str, context: dict[str, Any] | None
) -> tuple[list[dict[str, Any]], list[str]]:
    """Drop optional free-text arguments (plain `string`, no enum) whose value the user never wrote.

    Small models like to fill optional strings such as `dest` with made-up values; an invented
    export folder is worse than none (the UI asks when it is missing).
    """
    by = {t.name: t for t in tools}
    haystack = (message + " " + json.dumps(context or {}, ensure_ascii=False, default=str)).lower()
    notes: list[str] = []
    out: list[dict[str, Any]] = []
    for c in calls:
        sc = by[c["tool"]].schema
        props = sc.get("properties", {}) if isinstance(sc, dict) else {}
        req = set(sc.get("required", [])) if isinstance(sc, dict) else set()
        args = dict(c["args"])
        for k in list(args):
            p = props.get(k)
            v = args[k]
            if (
                isinstance(p, dict)
                and p.get("type") == "string"
                and "enum" not in p
                and k not in req
                and isinstance(v, str)
                and v.strip().lower() not in haystack
            ):
                del args[k]
                notes.append(f"dropped invented {c['tool']}.{k}={v!r}")
        out.append({"tool": c["tool"], "args": args})
    return out, notes
