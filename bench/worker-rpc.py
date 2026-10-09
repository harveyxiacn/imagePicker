"""Start the AI worker (dev venv) and run JSON-RPC calls against it; prints one JSON line per call.

Usage (from the repo root, with the worker's venv; the worker runs `uv run imagepicker-ai serve`):
    ai-worker/.venv/Scripts/python bench/worker-rpc.py [--models-dir DIR] [--device auto|cpu] \
        'system.info' 'models.list' 'models.ensure={"ids":["yunet"]}' ...
    ... --calls calls.json        # [{"method": ..., "params": {...}}, ...]

Each output line: {"method", "seconds", "result" | "error", "progress": <last progress params>}.
Used by bench/standard-eval-*.md to record `system.info` and to download / time single methods.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import secrets
import subprocess
import sys
import time
from pathlib import Path

import websockets


def parse_call(arg: str) -> dict:
    method, _, params = arg.partition("=")
    return {"method": method, "params": json.loads(params) if params else {}}


async def run(port: int, token: str, calls: list[dict], quiet_progress: bool) -> int:
    rc = 0
    uri = f"ws://127.0.0.1:{port}/?token={token}"
    async with websockets.connect(uri, max_size=None, ping_interval=None) as ws:
        for i, c in enumerate(calls, 1):
            t0 = time.perf_counter()
            await ws.send(json.dumps({"jsonrpc": "2.0", "id": i, "method": c["method"],
                                      "params": c.get("params", {})}))
            last_progress = None
            last_print = 0.0
            while True:
                msg = json.loads(await ws.recv())
                if msg.get("method") == "progress":
                    last_progress = msg.get("params")
                    if not quiet_progress and time.perf_counter() - last_print > 5:
                        print(json.dumps({"progress": last_progress}), file=sys.stderr, flush=True)
                        last_print = time.perf_counter()
                    continue
                if msg.get("id") != i:
                    continue
                out = {"method": c["method"], "seconds": round(time.perf_counter() - t0, 3)}
                if "error" in msg:
                    out["error"] = msg["error"]
                    rc = 1
                else:
                    out["result"] = msg.get("result")
                if last_progress is not None:
                    out["progress"] = last_progress
                print(json.dumps(out, ensure_ascii=False), flush=True)
                break
        try:
            await ws.send(json.dumps({"jsonrpc": "2.0", "id": 0, "method": "system.shutdown"}))
            await asyncio.wait_for(ws.recv(), 10)
        except Exception:
            pass
    return rc


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("calls", nargs="*", help="method or method=<json params>")
    ap.add_argument("--calls", dest="calls_file", type=Path)
    ap.add_argument("--models-dir")
    ap.add_argument("--device", default="auto")
    ap.add_argument("--worker-dir", default="ai-worker")
    ap.add_argument("--quiet-progress", action="store_true")
    a = ap.parse_args()
    calls = [parse_call(c) for c in a.calls]
    if a.calls_file:
        calls += json.loads(a.calls_file.read_text(encoding="utf-8"))
    token = secrets.token_hex(16)
    argv = ["uv", "run", "--no-sync", "imagepicker-ai", "serve", "--port", "0", "--token", token,
            "--device", a.device]
    if a.models_dir:
        argv += ["--models-dir", str(Path(a.models_dir).resolve())]
    proc = subprocess.Popen(argv, cwd=a.worker_dir, stdout=subprocess.PIPE, text=True,
                            env={**os.environ, "PYTHONUNBUFFERED": "1"})
    try:
        line = proc.stdout.readline()
        port = json.loads(line)["port"]
        return asyncio.run(run(port, token, calls, a.quiet_progress))
    finally:
        try:
            proc.wait(20)
        except subprocess.TimeoutExpired:
            proc.kill()


if __name__ == "__main__":
    sys.exit(main())
