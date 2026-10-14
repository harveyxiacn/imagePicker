"""Score natural-language photo search (`GET /api/search`) on the synthetic photo library.

Usage (the server must already run on --base; the library must be imported as one session and
analysed with the `standard` profile, so every photo has a SigLIP2 embedding; the text model
`siglip2-base-text` must be installed):

    python bench/eval-search.py bench/data/qwen-photo-library --session-id 1 \
        [--base http://127.0.0.1:7911] [--k 20] [--lang zh|en] [--json OUT.json] [--dry-run]

Each of the 20 concepts below is asked twice, once in English and once in Chinese (40 queries).
Relevance comes from the library manifest: a photo is relevant when the English prompt that
generated it mentions the concept (a regular expression over the scene description, the style
preamble is cut off) or when its tags say so. The prompts are intentions, not verified ground
truth (see `photo-library-2026-10-08.md`): a generated photo may miss a detail, so absolute recall
is underestimated and the numbers are for comparing zh with en and one model / threshold with
another.

Reported per query: relevant photos in the library, hits in the top K, precision@K, recall@K
(hits / relevant) and capped recall@K (hits / min(K, relevant), 1.0 = the best possible top K),
plus the mean of each per language and the score range of the top hit. `--min-score` defaults to 0
so the ranking, not the default threshold, is measured; pass the app default (0.05) to see what a
user would get.

`--dry-run` needs no server: it only lists the queries and how many manifest photos are relevant
to each. Standard library only.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Callable

# Same preamble as scripts/comfy-testset-v2.py; cut off so "no people / no text" do not match.
STYLE_RE = re.compile(
    r"^Realistic candid photograph shot on a modern mirrorless camera, natural skin texture, "
    r"true-to-life colours, no text, no watermark, no logos\.\s*"
)


def description(rec: dict) -> str:
    return STYLE_RE.sub("", rec.get("prompt", "")).lower()


def has(pattern: str) -> Callable[[dict], bool]:
    rx = re.compile(pattern)
    return lambda rec: bool(rx.search(description(rec)))


def tagged(*tags: str) -> Callable[[dict], bool]:
    return lambda rec: any(t in rec.get("tags", []) for t in tags)


@dataclass(frozen=True)
class Concept:
    key: str
    en: str
    zh: str
    relevant: Callable[[dict], bool]


CONCEPTS: list[Concept] = [
    Concept("beach", "a sandy beach by the sea", "海边的沙滩", has(r"\bbeach\b")),
    Concept("snow", "snow-covered ground in winter", "冬天的雪景", has(r"\bsnow")),
    Concept("cat", "a cat", "一只猫", has(r"\bcat\b|kitten|tabby")),
    Concept("dog", "a dog", "一只狗", has(r"\bdog\b|puppy|retriever")),
    Concept("noodles", "a bowl of noodle soup", "一碗热腾腾的面条", has(r"noodle")),
    Concept("cake", "a birthday cake with candles", "插着蜡烛的生日蛋糕", has(r"birthday cake|candles")),
    Concept("neon", "a street at night lit by neon signs", "霓虹灯下的夜晚街道", has(r"neon|night street|street at night")),
    Concept("group", "a group photo of several people", "一张多人合影", tagged("group")),
    Concept("family", "a family having dinner around a table", "一家人围着餐桌吃饭", tagged("family-dinner")),
    Concept("balloon", "a child holding a balloon", "拿着气球的小孩", has(r"balloon")),
    Concept("sakura", "cherry blossoms in full bloom", "盛开的樱花", has(r"cherry blossom")),
    Concept("torii", "red torii gates", "红色的鸟居", has(r"torii")),
    Concept("waterfall", "a waterfall in a mossy gorge", "山谷里的瀑布", has(r"waterfall")),
    Concept("lighthouse", "a lighthouse on the coast", "海岸边的灯塔", has(r"lighthouse")),
    Concept("bicycle", "a person riding a bicycle", "骑自行车的人", has(r"bicycle|\bbike\b|cycling")),
    Concept("umbrella", "an umbrella in the rain", "雨中撑着伞", has(r"umbrella")),
    Concept("wedding", "a wedding party on a lawn", "草坪上的婚礼", has(r"wedding party|bride")),
    Concept("document", "a printed document or receipt", "打印的文件或小票", tagged("document")),
    Concept("sunrise", "a landscape at sunrise with pink clouds", "日出时粉红色的云", has(r"sunrise")),
    Concept("hotpot", "a steaming hotpot", "冒着热气的火锅", has(r"hotpot")),
]


def queries(lang: str | None) -> list[tuple[str, str, str]]:
    """(concept key, language, query text)"""
    out = []
    for c in CONCEPTS:
        for lg, text in (("en", c.en), ("zh", c.zh)):
            if lang in (None, lg):
                out.append((c.key, lg, text))
    return out


def load_manifest(library: Path) -> dict[str, dict]:
    """JPEG file name (lower case) -> manifest record."""
    manifest = json.loads((library / "manifest.json").read_text(encoding="utf-8"))
    by_name = {}
    for rec in manifest["images"]:
        by_name[Path(rec["jpeg"].replace("\\", "/")).name.lower()] = rec
    return by_name


def relevant_names(by_name: dict[str, dict]) -> dict[str, set[str]]:
    return {c.key: {n for n, rec in by_name.items() if c.relevant(rec)} for c in CONCEPTS}


# ------------------------------------------------------------------ http
class Api:
    def __init__(self, base: str, cookie: str | None):
        self.base = base.rstrip("/")
        self.cookie = cookie

    def get(self, path: str):
        req = urllib.request.Request(self.base + path)
        if self.cookie:
            req.add_header("Cookie", self.cookie)
        try:
            with urllib.request.urlopen(req, timeout=300) as r:
                return r.status, json.loads(r.read())
        except urllib.error.HTTPError as e:
            body = e.read().decode("utf-8", "replace")
            try:
                body = json.loads(body)
            except ValueError:
                pass
            return e.code, body

    def photo_names(self, session_id: int) -> dict[int, str]:
        names: dict[int, str] = {}
        cursor = None
        while True:
            q = {"session_id": session_id, "limit": 5000}
            if cursor:
                q["cursor"] = cursor
            status, body = self.get("/api/photos?" + urllib.parse.urlencode(q))
            if status != 200:
                raise SystemExit(f"GET /api/photos failed: {status} {body}")
            for p in body["photos"]:
                names[p["id"]] = p["file_name"].lower()
            cursor = body.get("next_cursor")
            if not cursor:
                return names

    def search(self, session_id: int, text: str, k: int, min_score: float):
        q = urllib.parse.urlencode(
            {"q": text, "session_id": session_id, "limit": k, "min_score": min_score}
        )
        return self.get("/api/search?" + q)


# ------------------------------------------------------------------ scoring
def score_query(top: list[str], relevant: set[str], k: int) -> dict:
    hits = sum(1 for n in top[:k] if n in relevant)
    n_rel = len(relevant)
    return {
        "relevant": n_rel,
        "hits": hits,
        "precision": hits / max(1, len(top[:k])) if top else 0.0,
        "recall": hits / n_rel if n_rel else float("nan"),
        "recall_capped": hits / min(k, n_rel) if n_rel else float("nan"),
    }


def mean(xs: list[float]) -> float:
    xs = [x for x in xs if x == x]  # drop NaN
    return sum(xs) / len(xs) if xs else float("nan")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("library", type=Path, help="directory holding manifest.json")
    ap.add_argument("--session-id", type=int, help="session of the imported, standard-analysed library")
    ap.add_argument("--base", default="http://127.0.0.1:7911")
    ap.add_argument("--cookie", help='raw Cookie header when the server needs a login, e.g. "session=..."')
    ap.add_argument("--k", type=int, default=20)
    ap.add_argument("--lang", choices=["zh", "en"])
    ap.add_argument("--min-score", type=float, default=0.0)
    ap.add_argument("--json", type=Path, help="write the full results here")
    ap.add_argument("--dry-run", action="store_true", help="list queries and relevant counts; no server")
    a = ap.parse_args()

    by_name = load_manifest(a.library)
    rel = relevant_names(by_name)
    qs = queries(a.lang)
    print(f"manifest: {len(by_name)} images, {len(qs)} queries ({len(CONCEPTS)} concepts)")
    thin = [c.key for c in CONCEPTS if len(rel[c.key]) < 2]
    if thin:
        print(f"warning: fewer than 2 relevant photos for {', '.join(thin)} (library incomplete?)")

    if a.dry_run:
        print(f"\n{'concept':<11} {'lang':<4} {'relevant':>8}  query")
        for key, lg, text in qs:
            print(f"{key:<11} {lg:<4} {len(rel[key]):>8}  {text}")
        return 0
    if a.session_id is None:
        ap.error("--session-id is required unless --dry-run")

    api = Api(a.base, a.cookie)
    names = api.photo_names(a.session_id)
    known = set(by_name)
    missing = known - set(names.values())
    print(f"session {a.session_id}: {len(names)} photos, {len(missing)} manifest images not in it")

    rows = []
    print(f"\n{'concept':<11} {'lang':<4} {'rel':>4} {'hit':>4} {'P@K':>5} {'R@K':>5} {'R@K cap':>8} {'top score':>9}  query")
    unindexed = 0
    for key, lg, text in qs:
        status, body = api.search(a.session_id, text, a.k, a.min_score)
        if status == 409:
            print(f"search refused: {body}", file=sys.stderr)
            print("install the text model (siglip2-base-text) and re-run", file=sys.stderr)
            return 2
        if status != 200:
            print(f"search failed for {text!r}: {status} {body}", file=sys.stderr)
            return 1
        top = [names.get(h["photo_id"], "?") for h in body["hits"]]
        unindexed = body["unindexed"]
        r = score_query(top, rel[key] & set(names.values()), a.k)
        top_score = body["hits"][0]["score"] if body["hits"] else float("nan")
        rows.append({"concept": key, "lang": lg, "query": text, **r, "top_score": top_score,
                     "top": top[: a.k]})
        print(f"{key:<11} {lg:<4} {r['relevant']:>4} {r['hits']:>4} {r['precision']:>5.2f} "
              f"{r['recall']:>5.2f} {r['recall_capped']:>8.2f} {top_score:>9.3f}  {text}")

    summary = {}
    print()
    for lg in ("en", "zh"):
        sel = [r for r in rows if r["lang"] == lg]
        if not sel:
            continue
        summary[lg] = {
            "queries": len(sel),
            "precision": mean([r["precision"] for r in sel]),
            "recall": mean([r["recall"] for r in sel]),
            "recall_capped": mean([r["recall_capped"] for r in sel]),
            "queries_with_a_hit": sum(1 for r in sel if r["hits"] > 0),
        }
        s = summary[lg]
        print(f"{lg}: mean P@{a.k} {s['precision']:.3f}  R@{a.k} {s['recall']:.3f}  "
              f"capped R@{a.k} {s['recall_capped']:.3f}  queries with a hit {s['queries_with_a_hit']}/{s['queries']}")
    if unindexed:
        print(f"note: {unindexed} photos of the session have no embedding (run the standard analysis)")
    if a.json:
        a.json.write_text(json.dumps({"k": a.k, "min_score": a.min_score, "summary": summary, "queries": rows},
                                     ensure_ascii=False, indent=2), encoding="utf-8")
        print(f"wrote {a.json}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
