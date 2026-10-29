"""Assistant benchmark: plans natural-language commands through a running server and scores them.

Every command has an expected main tool, the arguments that matter (exposure +0.5 really is +0.5,
the named preset, the flag value, ...) and the photos it must act on; the target is checked by
the step's `affects` against `GET /api/photos` with the expected condition, so it does not depend
on how the plan spells it (`{"query"}`, a `filter` step + `current_filter`, ...). Only
`/api/assistant/plan` is called: nothing is executed, the catalog is not changed.

    python bench/eval-assistant.py --base http://127.0.0.1:17961 --engine both --json out.json

`--engine rules|llm|both`; `--cases all|base|new` (the 20 commands of the M6 benchmark, the
newer ones, or both). Failure kinds: `http_<status>` / `timeout` (request failed), `unsupported`
(no plan), `wrong_tool`, `wrong_args`, `wrong_target`. For `llm`, `fallback` notes plans that came
from the rules engine because the model plan was refused or failed; `model_ok` counts plans that
the model produced itself and that are correct.
"""

from __future__ import annotations

import argparse
import json
import math
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from typing import Any

SEL = "ids"  # target: the selected photos
ALL = "all"  # target: the current view = the whole session (no filter, nothing selected)

# (id, message, selected?, expected steps, target)
# expected steps: [(tool, args subset)]; the last one is the main step. `filter` steps before an
# action are not required (the target check covers the condition); a plan whose main tool is
# `filter` must match the expected filter arguments exactly (`sort` aside).
# target: SEL, ALL, a `/api/photos` query string (the step must affect exactly its matches),
# an int (that many photos), or None (no photos changed).
BASE: list[tuple[str, str, bool, list[tuple[str, dict[str, Any]]], Any]] = [
    ("b01", "把闭眼的照片都淘汰", False, [("set_flag", {"flag": -1})], "issues_any=closed_eyes"),
    ("b02", "reject every blurry photo", False, [("set_flag", {"flag": -1})], "issues_any=blurry"),
    (
        "b03",
        "每个场景只留1张，其他淘汰",
        False,
        [("scene_keep_top", {"n": 1, "reject_rest": True})],
        ALL,
    ),
    (
        "b04",
        "keep 2 per scene and reject the rest",
        False,
        [("scene_keep_top", {"n": 2, "reject_rest": True})],
        ALL,
    ),
    ("b05", "把前2张最好的评5星", False, [("set_rating", {"rating": 5})], 2),
    ("b06", "给所有照片打3星", False, [("set_rating", {"rating": 3})], ALL),
    ("b07", "只看5星", False, [("filter", {"rating_gte": 5})], None),
    (
        "b08",
        "show photos with closed eyes",
        False,
        [("filter", {"issues_any": ["closed_eyes"]})],
        None,
    ),
    ("b09", "把选中的照片淘汰", True, [("set_flag", {"flag": -1})], SEL),
    ("b10", "reject the selected photos", True, [("set_flag", {"flag": -1})], SEL),
    ("b11", "把选中的照片设为精选", True, [("set_flag", {"flag": 1})], SEL),
    ("b12", "pick the selected photos", True, [("set_flag", {"flag": 1})], SEL),
    ("b13", "给选中的照片打4星", True, [("set_rating", {"rating": 4})], SEL),
    ("b14", "rate selected photos 4 stars", True, [("set_rating", {"rating": 4})], SEL),
    ("b15", "只看模糊的照片", False, [("filter", {"issues_any": ["blurry"]})], None),
    ("b16", "show rejected photos", False, [("filter", {"flag": "rejected"})], None),
    (
        "b17",
        "给选中的照片应用暖调胶片预设",
        True,
        [("apply_preset", {"preset_id": "film_warm"})],
        SEL,
    ),
    (
        "b18",
        "apply film warm preset to selected photos",
        True,
        [("apply_preset", {"preset_id": "film_warm"})],
        SEL,
    ),
    (
        "b19",
        "给选中的照片增加0.5曝光",
        True,
        [("adjust_sliders", {"values": {"exposure": 0.5}, "relative": True})],
        SEL,
    ),
    (
        "b20",
        "increase exposure by 0.5",
        False,
        [("adjust_sliders", {"values": {"exposure": 0.5}, "relative": True})],
        ALL,
    ),
]

NEW: list[tuple[str, str, bool, list[tuple[str, dict[str, Any]]], Any]] = [
    ("n01", "把闭眼的都标成淘汰", False, [("set_flag", {"flag": -1})], "issues_any=closed_eyes"),
    (
        "n02",
        "show only 4 stars and up from the beach",
        False,
        # no beach condition exists: the only honest plans are a 4-star filter that says the
        # place was ignored, or no plan at all; see `HONEST_PARTIAL`
        [("filter", {"rating_gte": 4})],
        None,
    ),
    (
        "n03",
        "只看2026年10月2日拍的照片",
        False,
        [("filter", {"taken_from": "2026-10-02", "taken_to": "2026-10-02"})],
        None,
    ),
    (
        "n04",
        "show photos taken on 2026-10-01",
        False,
        [("filter", {"taken_from": "2026-10-01", "taken_to": "2026-10-01"})],
        None,
    ),
    ("n05", "只看有三个人以上的照片", False, [("filter", {"faces_min": 3})], None),
    ("n06", "show photos with nobody in them", False, [("filter", {"faces_max": 0})], None),
    (
        "n07",
        "show the group photos where everyone's eyes are open",
        False,
        [("filter", {"scene_type": "group", "person_state": ["eyes_open"]})],
        None,
    ),
    (
        "n08",
        "把过曝和欠曝的都淘汰",
        False,
        [("set_flag", {"flag": -1})],
        "issues_any=overexposed,underexposed",
    ),
    (
        "n09",
        "把夜景照片的曝光提高0.3",
        False,
        [("adjust_sliders", {"values": {"exposure": 0.3}, "relative": True})],
        "scene_type=night",
    ),
    ("n10", "清除选中照片的旗标", True, [("set_flag", {"flag": 0})], SEL),
    (
        "n11",
        "export the 5-star photos for WeChat",
        False,
        [("export", {"preset": "wechat"})],
        "rating_gte=5",
    ),
    (
        "n12",
        "lower the contrast of the selected photos by 10",
        True,
        [("adjust_sliders", {"values": {"contrast": -10}, "relative": True})],
        SEL,
    ),
    ("n13", "只看没有修过的照片", False, [("filter", {"has_edits": False})], None),
    ("n14", "把人像照片都评4星", False, [("set_rating", {"rating": 4})], "scene_type=portrait"),
]

# commands whose condition cannot be expressed: a plan that keeps the rest of the request and
# says what it ignored (a hint / the reply) is the best possible answer; `unsupported` is accepted
HONEST_PARTIAL = {"n02"}

CHANGES = {
    "set_rating",
    "set_flag",
    "accept_ai",
    "group_keep_top",
    "scene_keep_top",
    "apply_preset",
    "auto_adjust",
    "adjust_sliders",
    "apply_profiles",
    "besttake_auto",
    "remove_bystanders",
    "export",
}


def has_cjk(s: str) -> bool:
    return any("一" <= ch <= "鿿" for ch in s)


class Api:
    def __init__(self, base: str, timeout: float):
        self.base = base.rstrip("/")
        self.timeout = timeout

    def get(self, path: str) -> Any:
        with urllib.request.urlopen(self.base + path, timeout=30) as r:
            return json.load(r)

    def post(self, path: str, body: Any) -> Any:
        req = urllib.request.Request(
            self.base + path,
            data=json.dumps(body).encode(),
            headers={"Content-Type": "application/json"},
        )
        with urllib.request.urlopen(req, timeout=self.timeout) as r:
            return json.load(r)

    def count(self, session: int, query: str) -> int:
        q = f"session_id={session}&limit=1" + (f"&{query}" if query else "")
        return int(self.get("/api/photos?" + q)["total"])


def num_eq(a: Any, b: Any) -> bool:
    if isinstance(a, bool) or isinstance(b, bool):
        return a is b
    if isinstance(a, (int, float)) and isinstance(b, (int, float)):
        return math.isclose(float(a), float(b), abs_tol=1e-6)
    return False


def matches(want: Any, got: Any) -> bool:
    """`want` is a subset of `got` (dicts), equal as sets (lists) or equal (numbers / strings)."""
    if isinstance(want, dict):
        return isinstance(got, dict) and all(
            k in got and matches(v, got[k]) for k, v in want.items()
        )
    if isinstance(want, list):
        return isinstance(got, list) and sorted(map(json.dumps, want)) == sorted(
            map(json.dumps, got)
        )
    if isinstance(want, (int, float)) and not isinstance(want, bool):
        return num_eq(want, got)
    return want == got


def filter_equal(want: dict[str, Any], got: dict[str, Any]) -> bool:
    g = {k: v for k, v in got.items() if k != "sort"}
    w = {k: v for k, v in want.items() if k != "sort"}
    return set(g) == set(w) and matches(w, g)


def selection_is_ids(args: dict[str, Any], ids: list[int]) -> bool:
    s = args.get("selection")
    return isinstance(s, dict) and sorted(s.get("ids") or []) == sorted(ids)


def score(case, plan: dict[str, Any], api: Api, session: int, ids: list[int], total: int, cache):
    cid, _msg, _sel, expect, target = case
    steps = plan.get("steps") or []
    main_tool, main_args = expect[-1]
    if plan.get("unsupported") or not steps:
        return False, False, "unsupported"
    changing = [s for s in steps if s["tool"] in CHANGES]
    if main_tool == "filter":
        tool_ok = not changing and any(s["tool"] == "filter" for s in steps)
        if not tool_ok:
            return False, False, "wrong_tool"
        last = [s for s in steps if s["tool"] == "filter"][-1]
        return (
            True,
            filter_equal(main_args, last["args"]),
            (None if filter_equal(main_args, last["args"]) else "wrong_args"),
        )
    tool_ok = [s["tool"] for s in changing] == [main_tool]
    if not tool_ok:
        return False, False, "wrong_tool"
    step = changing[0]
    if not matches(main_args, step["args"]):
        return True, False, "wrong_args"
    if target == SEL:
        ok = selection_is_ids(step["args"], ids)
    elif target == ALL:
        keep_top = main_tool in ("group_keep_top", "scene_keep_top")
        sel = step["args"].get("selection")
        unscoped = (
            sel is None
            or sel == "current_filter"
            or (
                isinstance(sel, dict)
                and "query" in sel
                and not any(k not in ("sort", "limit") for k in urllib.parse.parse_qs(sel["query"]))
            )
        )
        ok = unscoped and (keep_top or step["affects"] == total)
    elif isinstance(target, int):
        ok = step["affects"] == target
    else:
        if target not in cache:
            cache[target] = api.count(session, target)
        ok = step["affects"] == cache[target]
    return True, ok, None if ok else "wrong_target"


def pct(xs: list[float], p: float) -> float:
    if not xs:
        return float("nan")
    s = sorted(xs)
    k = (len(s) - 1) * p
    lo, hi = math.floor(k), math.ceil(k)
    return s[lo] + (s[hi] - s[lo]) * (k - lo)


def run(
    api: Api, engine: str, cases, session: int, ids: list[int], total: int, cache
) -> list[dict]:
    rows = []
    for case in cases:
        cid, msg, selected, expect, target = case
        body = {
            "session_id": session,
            "message": msg,
            "engine": engine,
            "context": {
                "filter": "",
                "selection": ids if selected else [],
                "current_photo_id": None,
                "locale": "zh-CN" if has_cjk(msg) else "en",
            },
        }
        t0 = time.monotonic()
        row: dict[str, Any] = {"id": cid, "engine": engine, "message": msg, "expect": expect[-1][0]}
        try:
            plan = api.post("/api/assistant/plan", body)
        except urllib.error.HTTPError as e:
            detail = e.read().decode("utf-8", "replace")[:300]
            row.update(error=f"http_{e.code}", detail=detail, tool_ok=False, args_ok=False)
        except (TimeoutError, urllib.error.URLError) as e:
            row.update(error="timeout" if "timed out" in str(e) else "conn", detail=str(e))
            row.update(tool_ok=False, args_ok=False)
        else:
            tool_ok, args_ok, kind = score(case, plan, api, session, ids, total, cache)
            if cid in HONEST_PARTIAL:
                hinted = bool(plan.get("unsupported")) or any(
                    w in (plan.get("reply") or "").lower()
                    for w in ("ignor", "忽略", "beach", "海滩")
                )
                if not hinted:
                    tool_ok, args_ok, kind = tool_ok, False, kind or "silent_drop"
                elif plan.get("unsupported"):
                    tool_ok, args_ok, kind = True, True, None
            row.update(
                tool_ok=tool_ok,
                args_ok=tool_ok and args_ok,
                kind=kind,
                engine_used=plan.get("engine"),
                fallback=(plan.get("fallback") or {}).get("code"),
                steps=[
                    {"tool": s["tool"], "args": s["args"], "affects": s["affects"]}
                    for s in plan.get("steps") or []
                ],
                reply=plan.get("reply"),
                unsupported=plan.get("unsupported"),
            )
        row["seconds"] = round(time.monotonic() - t0, 3)
        row["kind"] = row.get("kind") or row.get("error")
        rows.append(row)
        flag = "ok " if row["args_ok"] else ("~  " if row["tool_ok"] else "XX ")
        extra = f" fb={row.get('fallback')}" if row.get("fallback") else ""
        print(
            f"{flag}{engine:5} {cid} {row['seconds']:6.2f}s {row.get('kind') or '':14}{extra} | {msg}",
            flush=True,
        )
    return rows


def summary(rows: list[dict]) -> dict[str, Any]:
    n = len(rows)
    lat = [r["seconds"] for r in rows if not r.get("error")]
    kinds: dict[str, int] = {}
    for r in rows:
        if r.get("kind"):
            kinds[r["kind"]] = kinds.get(r["kind"], 0) + 1
    return {
        "n": n,
        "tool_correct": sum(r["tool_ok"] for r in rows),
        "args_correct": sum(r["args_ok"] for r in rows),
        "tool_pct": round(100 * sum(r["tool_ok"] for r in rows) / n, 1) if n else 0,
        "args_pct": round(100 * sum(r["args_ok"] for r in rows) / n, 1) if n else 0,
        "model_ok": sum(r["args_ok"] and r.get("engine_used") == "llm" for r in rows),
        "fallbacks": sum(bool(r.get("fallback")) for r in rows),
        "errors": sum(bool(r.get("error")) for r in rows),
        "p50_s": round(pct(lat, 0.5), 3) if lat else None,
        "p95_s": round(pct(lat, 0.95), 3) if lat else None,
        "kinds": kinds,
    }


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument("--base", default="http://127.0.0.1:17961", help="server URL")
    ap.add_argument("--engine", choices=["rules", "llm", "both"], default="both")
    ap.add_argument("--cases", choices=["all", "base", "new"], default="all")
    ap.add_argument("--session", type=int, help="session id (default: the one with most photos)")
    ap.add_argument("--timeout", type=float, default=180, help="seconds per plan request")
    ap.add_argument("--no-warmup", action="store_true", help="llm: do not load the model first")
    ap.add_argument("--json", help="write rows and the summary here")
    a = ap.parse_args()
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8")

    api = Api(a.base, a.timeout)
    sessions = api.get("/api/sessions")["sessions"]
    if not sessions:
        print("no session in this catalog", file=sys.stderr)
        return 2
    session = a.session or max(sessions, key=lambda s: s.get("photo_count", 0))["id"]
    ids = [p["id"] for p in api.get(f"/api/photos?session_id={session}&limit=3")["photos"]]
    total = api.count(session, "")
    cases = {"all": BASE + NEW, "base": BASE, "new": NEW}[a.cases]
    print(f"session {session}: {total} photos, selection {ids}, {len(cases)} commands", flush=True)

    cache: dict[str, int] = {}
    out: dict[str, Any] = {"base": a.base, "session": session, "photos": total, "engines": {}}
    for engine in ["rules", "llm"] if a.engine == "both" else [a.engine]:
        warm = None
        if engine == "llm" and not a.no_warmup:
            t0 = time.monotonic()
            try:
                api.post(
                    "/api/assistant/plan",
                    {"session_id": session, "message": "只看5星", "engine": "llm", "context": {}},
                )
                warm = {"seconds": round(time.monotonic() - t0, 2)}
            except Exception as e:  # noqa: BLE001
                warm = {"seconds": round(time.monotonic() - t0, 2), "error": str(e)[:200]}
            print(f"warm-up: {warm}", flush=True)
        rows = run(api, engine, cases, session, ids, total, cache)
        by = {
            "all": summary(rows),
            "base": summary([r for r in rows if r["id"].startswith("b")]),
            "new": summary([r for r in rows if r["id"].startswith("n")]),
        }
        out["engines"][engine] = {"warmup": warm, "summary": by, "rows": rows}
        for k, s in by.items():
            if s["n"]:
                print(
                    f"== {engine} {k}: tool {s['tool_correct']}/{s['n']} ({s['tool_pct']}%), "
                    f"args {s['args_correct']}/{s['n']} ({s['args_pct']}%), model-own {s['model_ok']}, "
                    f"fallbacks {s['fallbacks']}, errors {s['errors']}, p50 {s['p50_s']}s, "
                    f"p95 {s['p95_s']}s, kinds {s['kinds']}",
                    flush=True,
                )
        if a.json:
            with open(a.json, "w", encoding="utf-8") as f:
                json.dump(out, f, ensure_ascii=False, indent=2)
    return 0


if __name__ == "__main__":
    sys.exit(main())
