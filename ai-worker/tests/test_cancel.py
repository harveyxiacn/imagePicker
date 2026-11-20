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


# ---------------------------------------------------------------- besttake.compose / hair.compose
def _stage_log():
    """Fake Best Take components that log every call; the log is shared with the patched aligners."""
    from imagepicker_ai.besttake.compose import Components
    from imagepicker_ai.masks.nets import CLS_FACE_SKIN

    from .synth_face import observer

    log: list[str] = []
    obs = observer()

    def observe(rgb, box):
        log.append("observe")
        return obs(rgb, box)

    def parts(crop):
        log.append("parts")
        probs = np.zeros((*crop.shape[:2], 6), np.float32)
        probs[..., CLS_FACE_SKIN] = 1.0
        return probs

    def matte(win, face, others):
        log.append("matte")
        return np.ones(win.shape[:2], np.float32)

    def detect(rgb):
        log.append("detect")
        return []

    return log, Components(observe, parts, matte, detect)


def _logging_aligners(monkeypatch, log):
    from imagepicker_ai.besttake import align

    real_global, real_flow = align.align_global, align.refine_flow

    def align_global(*a, **k):
        log.append("align_global")
        return real_global(*a, **k)

    def refine_flow(*a, **k):
        log.append("refine_flow")
        return real_flow(*a, **k)

    monkeypatch.setattr(align, "align_global", align_global)
    monkeypatch.setattr(align, "refine_flow", refine_flow)


def test_besttake_compose_stops_before_every_later_stage(monkeypatch):
    from imagepicker_ai.besttake.compose import compose_arrays

    from .test_m5 import make_pair

    base, src, _truth, bf, sf = make_pair()
    log, comp = _stage_log()
    _logging_aligners(monkeypatch, log)
    full = compose_arrays(base, src, bf, sf, comp)
    stages = list(log)
    assert stages[:2] == ["observe", "observe"] and "refine_flow" in stages
    assert stages.count("parts") == 2 and stages.count("matte") == 2
    assert stages.count("detect") == 2 and "align_global" in stages

    for n in range(len(stages)):  # cancel once n stages have run: stage n + 1 must not start
        log.clear()

        def check(n=n) -> None:
            if len(log) >= n:
                raise Cancelled

        with pytest.raises(Cancelled):
            compose_arrays(base, src, bf, sf, comp, check)
        assert log == stages[:n], f"stage {n + 1} ({stages[n]}) ran after the cancel"

    # an uncancelled compose with a check is byte-identical to one without
    log.clear()
    again = compose_arrays(base, src, bf, sf, comp, lambda: None)
    assert full.patch is not None and again.patch is not None
    assert full.rect == again.rect
    assert full.patch.tobytes() == again.patch.tobytes()


def test_besttake_cancel_is_not_swallowed_by_the_landmark_recheck():
    """`_landmark_error` tolerates a failing observe; a cancel must still unwind from there."""
    from imagepicker_ai.besttake.compose import compose_arrays

    from .test_m5 import make_pair

    base, src, _truth, bf, sf = make_pair()
    log, comp = _stage_log()
    compose_arrays(base, src, bf, sf, comp)
    total = len(log)
    log.clear()

    def check() -> None:
        if len(log) >= total - 1:  # only the final landmark re-detection is left
            raise Cancelled

    with pytest.raises(Cancelled):
        compose_arrays(base, src, bf, sf, comp, check)


def test_besttake_cancel_releases_through_the_service_retry():
    """`_run_retry` must not turn a mid-run Cancelled into an OOM retry or swallow it."""
    from imagepicker_ai.besttake.service import BestTake

    class Boom(BestTake):
        def __init__(self) -> None:
            self.calls = 0

        def _run(self, req, check=None):
            self.calls += 1
            raise Cancelled

    bt = Boom()
    with pytest.raises(Cancelled):
        bt._run_retry({}, lambda: None)
    assert bt.calls == 1


def test_hair_compose_stops_before_every_later_stage():
    from imagepicker_ai.besttake.compose import Components
    from imagepicker_ai.besttake.hair import hair_compose_arrays

    from .synth_face import observer
    from .test_m5 import make_pair

    base, src, _truth, bf, sf = make_pair()
    log: list[str] = []
    obs = observer()

    def observe(rgb, box):
        log.append("observe")
        return obs(rgb, box)

    def parts(crop):
        log.append("parts")
        return np.zeros((*crop.shape[:2], 6), np.float32)

    comp = Components(observe, parts)
    hair_compose_arrays(base, src, bf, sf, comp)
    stages = list(log)
    assert stages == ["observe", "observe", "parts", "parts"]
    for n in range(len(stages)):
        log.clear()

        def check(n=n) -> None:
            if len(log) >= n:
                raise Cancelled

        with pytest.raises(Cancelled):
            hair_compose_arrays(base, src, bf, sf, comp, None, check)
        assert log == stages[:n]
