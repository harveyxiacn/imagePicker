"""Benchmark + quality check of the local assistant (`llm.plan`, `vlm.suggest`, `vlm.describe`).

  uv run python scripts/bench_assistant.py [--device auto|cpu] [--runs 3] [--llm ID] [--vlm ID]
                                           [--image photo.jpg] [--json out.json] [--verbose]

LLM: the 15 zh/en commands of tests/assistant_fixtures.py (average latency, how many produce the
expected tool calls). VLM: suggest on a normal and on a darkened copy of `--image`, describe in en and
zh (median of --runs after one warm-up). Reports VRAM used by each model (nvidia-smi delta). Models
are downloaded into the models dir when missing.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import statistics
import sys
import tempfile
import time
from pathlib import Path

import cv2
import numpy as np

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(ROOT / "tests"))

from assistant_fixtures import COMMANDS, CONTEXT, TOOLS, matches  # noqa: E402

from imagepicker_ai import hw as hwmod  # noqa: E402
from imagepicker_ai.models import Downloader  # noqa: E402
from imagepicker_ai.service import WorkerService  # noqa: E402


def gpu_used() -> int | None:
    m = hwmod.nvidia_memory_usage()
    return m["used_mb"] if m else None


def bench_llm(svc: WorkerService, a: argparse.Namespace) -> dict:
    out: dict = {}
    extra = {"model": a.llm} if a.llm else {}
    base = gpu_used()
    t = time.perf_counter()
    asyncio.run(svc.assistant.plan(_req("把当前筛选的照片全部评为5星", "zh-CN", extra)))
    out["load_plus_first_plan_s"] = round(time.perf_counter() - t, 2)
    after = gpu_used()
    out["vram_mb"] = (after - base) if base is not None and after is not None else None
    lat, ok, rows = [], 0, []
    for msg, loc, expected in COMMANDS:
        t = time.perf_counter()
        r = asyncio.run(svc.assistant.plan(_req(msg, loc, extra)))
        dt = time.perf_counter() - t
        good = matches(r["calls"], expected)
        ok += good
        lat.append(dt)
        out["model"] = r["model"]
        rows.append({"message": msg, "ok": good, "s": round(dt, 2), "calls": r["calls"],
                     "repaired": r["repaired"]})  # fmt: skip
        if a.verbose or not good:
            print(
                ("OK  " if good else "FAIL"),
                f"{dt:5.2f}s",
                msg,
                json.dumps(r["calls"], ensure_ascii=False),
            )
    out["quality"] = f"{ok}/{len(COMMANDS)}"
    out["avg_s"] = round(statistics.mean(lat), 3)
    out["median_s"] = round(statistics.median(lat), 3)
    out["max_s"] = round(max(lat), 3)
    out["rows"] = rows
    return out


def _req(msg: str, locale: str, extra: dict) -> dict:
    return {"message": msg, "tools": TOOLS, "context": CONTEXT, "locale": locale, **extra}


def bench_vlm(svc: WorkerService, a: argparse.Namespace) -> dict:
    extra = {"model": a.vlm} if a.vlm else {}
    tmp = Path(tempfile.mkdtemp(prefix="bench_assistant_"))
    img = cv2.imread(a.image)
    dark = np.clip(img.astype(np.float32) * 0.15, 0, 255).astype(np.uint8)
    cv2.imwrite(str(tmp / "dark.jpg"), dark)
    normal = {"photo_id": 1, "path": a.image}
    darkp = {"photo_id": 2, "path": str(tmp / "dark.jpg")}
    out: dict = {}

    def timed(coro_fn) -> tuple[float, dict]:
        asyncio.run(coro_fn())  # warm-up
        ts = []
        r = {}
        for _ in range(a.runs):
            t = time.perf_counter()
            r = asyncio.run(coro_fn())
            ts.append(time.perf_counter() - t)
        return statistics.median(ts), r

    base = gpu_used()
    t = time.perf_counter()
    asyncio.run(svc.assistant.suggest({"photo": normal, "locale": "en", **extra}))
    out["load_plus_first_suggest_s"] = round(time.perf_counter() - t, 2)
    after = gpu_used()
    out["vram_mb"] = (after - base) if base is not None and after is not None else None
    for name, photo in (("normal", normal), ("dark", darkp)):
        s, r = timed(lambda p=photo: svc.assistant.suggest({"photo": p, "locale": "en", **extra}))
        out[f"suggest_{name}_s"] = round(s, 3)
        out[f"suggest_{name}"] = {k: r[k] for k in ("problems", "adjust", "reason")}
        out["model"] = r["model"]
    for loc in ("en", "zh-CN"):
        s, r = timed(
            lambda lo=loc: svc.assistant.describe({"photo": normal, "locale": lo, **extra})
        )
        out[f"describe_{loc}_s"] = round(s, 3)
        out[f"describe_{loc}"] = {k: r[k] for k in ("caption", "keywords")}
    return out


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--device", default="auto")
    ap.add_argument("--runs", type=int, default=3)
    ap.add_argument("--llm")
    ap.add_argument("--vlm")
    ap.add_argument("--skip-llm", action="store_true")
    ap.add_argument("--skip-vlm", action="store_true")
    ap.add_argument("--verbose", action="store_true")
    ap.add_argument("--image", default=str(ROOT / ".models/_testimg/Eiffel_Tower_20051010.jpg"))
    ap.add_argument("--models-dir", default=str(ROOT / ".models"))
    ap.add_argument("--json")
    a = ap.parse_args()

    svc = WorkerService(models_dir=a.models_dir, device=a.device, idle_unload_s=0)
    svc.assistant.any_tier = True  # benchmarks may run on any hardware
    d = Downloader(Path(a.models_dir))
    roles = [r for r, skip in (("llm", a.skip_llm), ("vlm", a.skip_vlm)) if not skip]
    for role in roles:
        want = getattr(a, role) or (svc.assistant.candidates(role) or [None])[0]
        spec = svc.registry.get(want) if isinstance(want, str) else want
        if spec is not None:
            d.ensure(spec)
    res: dict = {
        "hardware": {"tier": svc.hw.tier, "device": svc.hw.device, "providers": svc.hw.providers}
    }
    if not a.skip_llm:
        res["llm"] = bench_llm(svc, a)
        svc.manager.unload()
    if not a.skip_vlm:
        res["vlm"] = bench_vlm(svc, a)
    summary = json.dumps(
        {
            k: ({kk: vv for kk, vv in v.items() if kk != "rows"} if isinstance(v, dict) else v)
            for k, v in res.items()
        },  # fmt: skip
        indent=2,
        ensure_ascii=False,
    )
    print(summary)
    if a.json:
        Path(a.json).write_text(json.dumps(res, indent=2, ensure_ascii=False), encoding="utf-8")
    asyncio.run(svc.stop())


if __name__ == "__main__":
    main()
