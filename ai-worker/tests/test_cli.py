"""Spawn the real CLI the way the Rust core will."""

import asyncio
import json
import os
import subprocess
import sys
import threading

import pytest
from websockets.asyncio.client import connect
from websockets.exceptions import InvalidStatus


def _spawn(tmp_path, *extra, env_extra=None):
    env = {**os.environ, **(env_extra or {})}
    return subprocess.Popen(
        [
            sys.executable,
            "-m",
            "imagepicker_ai",
            "serve",
            "--models-dir",
            str(tmp_path / "m"),
            "--device",
            "cpu",
            *extra,
        ],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        env=env,
        creationflags=getattr(subprocess, "CREATE_NEW_PROCESS_GROUP", 0),
    )


def _ready(proc, timeout=60):
    box = []
    t = threading.Thread(target=lambda: box.append(proc.stdout.readline()), daemon=True)
    t.start()
    t.join(timeout)
    assert box, "worker did not print ready line"
    return json.loads(box[0])


async def _info(port, token):
    async with connect(
        f"ws://127.0.0.1:{port}", additional_headers={"Authorization": f"Bearer {token}"}
    ) as ws:
        await ws.send(json.dumps({"jsonrpc": "2.0", "id": 1, "method": "system.info"}))
        return json.loads(await ws.recv())["result"]


def test_ready_line_token_and_shutdown_rpc(tmp_path):
    p = _spawn(tmp_path, "--host", "127.0.0.1", "--port", "0", "--token", "T")
    try:
        ev = _ready(p)
        assert ev["event"] == "ready" and isinstance(ev["port"], int) and ev["port"] > 0
        info = asyncio.run(_info(ev["port"], "T"))
        assert info["pid"] > 0  # (venv launcher shims on Windows make this differ from p.pid)

        async def bad():
            with pytest.raises(InvalidStatus):
                await connect(
                    f"ws://127.0.0.1:{ev['port']}", additional_headers={"Authorization": "Bearer X"}
                )

        asyncio.run(bad())

        async def stop():
            async with connect(
                f"ws://127.0.0.1:{ev['port']}", additional_headers={"Authorization": "Bearer T"}
            ) as ws:
                await ws.send(json.dumps({"jsonrpc": "2.0", "id": 2, "method": "system.shutdown"}))
                return json.loads(await ws.recv())

        assert asyncio.run(stop())["result"]["shutting_down"] is True
        assert p.wait(timeout=20) == 0
    finally:
        if p.poll() is None:
            p.kill()


def test_env_addr_and_token(tmp_path):
    p = _spawn(tmp_path, env_extra={"IP_WORKER_ADDR": "127.0.0.1:0", "IP_WORKER_TOKEN": "envtok"})
    try:
        ev = _ready(p)
        assert asyncio.run(_info(ev["port"], "envtok"))["protocol"] == 1
    finally:
        p.kill()


def test_refuses_to_start_without_token(tmp_path):
    env = {k: v for k, v in os.environ.items() if k != "IP_WORKER_TOKEN"}
    r = subprocess.run(
        [sys.executable, "-m", "imagepicker_ai", "serve", "--models-dir", str(tmp_path / "m")],
        capture_output=True,
        text=True,
        env=env,
        timeout=60,
    )
    assert r.returncode == 2
    assert "token" in r.stderr
    assert r.stdout.strip() == ""
