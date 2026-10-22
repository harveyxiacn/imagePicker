"""Cancellation reaches the thread-pool work, not only the asyncio task."""

import asyncio
import json
import threading
import time

import numpy as np
import pytest
from websockets.asyncio.client import connect

from imagepicker_ai import errors
from imagepicker_ai.cancel import Cancel, Cancelled
from imagepicker_ai.rpc import Ctx, RpcServer

TOKEN = "s3cret"


def test_cancel_flag_only_raises_once_set():
    c = Cancel()
    c.check()  # no flag: a no-op, so callers can call it unconditionally
    c.set()
    assert c.is_set()
    with pytest.raises(Cancelled):
        c.check()


async def test_cancel_stops_the_executor_thread_before_it_finishes():
    """The RPC answers -32800 at once; the pool thread must notice and release itself."""
    started = threading.Event()
    exited = threading.Event()
    outcome: dict[str, bool] = {}

    def work(ctx: Ctx) -> dict[str, bool]:
        started.set()
        try:
            for _ in range(400):  # 2 s of "inference" in 5 ms steps
                ctx.cancel.check()
                time.sleep(0.005)
            outcome["finished"] = True
            return {"finished": True}
        finally:
            exited.set()

    async def h_job(params, ctx: Ctx):
        return await asyncio.get_running_loop().run_in_executor(None, work, ctx)

    srv = RpcServer({"job": h_job}, TOKEN)
    port = await srv.start("127.0.0.1", 0)
    try:
        async with await connect(
            f"ws://127.0.0.1:{port}/", additional_headers={"Authorization": f"Bearer {TOKEN}"}
        ) as ws:
            await ws.send(json.dumps({"jsonrpc": "2.0", "id": 1, "method": "job"}))
            loop = asyncio.get_running_loop()
            assert await loop.run_in_executor(None, started.wait, 5), "work never started"
            t0 = time.monotonic()
            await ws.send(json.dumps({"jsonrpc": "2.0", "method": "cancel", "params": {"req": 1}}))
            r = json.loads(await asyncio.wait_for(ws.recv(), 5))
            assert r["id"] == 1 and r["error"]["code"] == errors.CANCELLED
            assert await loop.run_in_executor(None, exited.wait, 5), "thread never exited"
            assert time.monotonic() - t0 < 1.0, "the thread kept computing after the cancel"
    finally:
        await srv.stop()
    assert "finished" not in outcome, "the work ran to the end despite the cancel"


def test_inpaint_stops_between_clusters():
    from imagepicker_ai.inpaint.core import FillEngine, inpaint_arrays

    calls = {"n": 0}

    def fill(img: np.ndarray, mask: np.ndarray) -> np.ndarray:
        calls["n"] += 1
        return img.copy()

    engine = FillEngine(512, fill)
    img = np.full((256, 256, 3), 90, np.uint8)
    mask = np.zeros((256, 256), np.float32)
    mask[20:40, 20:40] = 1.0
    mask[200:220, 200:220] = 1.0  # two clusters, far further apart than the 64 px gap

    out = inpaint_arrays(img, mask, engine)
    assert out is not None and calls["n"] == 2

    calls["n"] = 0

    def check() -> None:
        if calls["n"] >= 1:
            raise Cancelled

    with pytest.raises(Cancelled):
        inpaint_arrays(img, mask, engine, check=check)
    assert calls["n"] == 1, "the second cluster was still filled after the cancel"
