"""JSON-RPC protocol tests against an in-process server on port 0."""

import asyncio
import json
import time

import pytest
import pytest_asyncio
from websockets.asyncio.client import connect
from websockets.exceptions import InvalidStatus

from imagepicker_ai import errors
from imagepicker_ai.errors import InvalidParams, RpcError
from imagepicker_ai.rpc import Ctx, RpcServer

TOKEN = "s3cret"


async def h_echo(params, ctx: Ctx):
    return {"echo": params}


async def h_sleep(params, ctx: Ctx):
    await asyncio.sleep(params["s"])
    return {"slept": params["s"]}


async def h_progress(params, ctx: Ctx):
    for i in range(1, params["n"] + 1):
        ctx.progress(done=i, total=params["n"])
        await asyncio.sleep(0)
    return {"ok": True}


async def h_invalid(params, ctx: Ctx):
    raise InvalidParams("bad thing", {"field": "x"})


async def h_custom(params, ctx: Ctx):
    raise RpcError(-32010, "no model", "model_unavailable", {"models": ["m"]})


async def h_boom(params, ctx: Ctx):
    raise RuntimeError("kaboom")


async def h_forever(params, ctx: Ctx):
    await asyncio.sleep(60)


@pytest_asyncio.fixture
async def server():
    srv = RpcServer(
        {
            "echo": h_echo,
            "sleep": h_sleep,
            "progress": h_progress,
            "invalid": h_invalid,
            "custom": h_custom,
            "boom": h_boom,
            "forever": h_forever,
        },
        TOKEN,
    )
    port = await srv.start("127.0.0.1", 0)
    assert port > 0
    yield srv, port
    await srv.stop()


def url(port, q=""):
    return f"ws://127.0.0.1:{port}/{q}"


async def client(port):
    return await connect(url(port), additional_headers={"Authorization": f"Bearer {TOKEN}"})


async def call(ws, method, params=None, id=1):
    msg = {"jsonrpc": "2.0", "id": id, "method": method}
    if params is not None:
        msg["params"] = params
    await ws.send(json.dumps(msg))


async def recv(ws, timeout=5):
    return json.loads(await asyncio.wait_for(ws.recv(), timeout))


async def test_auth_missing_token_rejected(server):
    _, port = server
    with pytest.raises(InvalidStatus) as ei:
        await connect(url(port))
    assert ei.value.response.status_code == 401


async def test_auth_wrong_token_rejected(server):
    _, port = server
    with pytest.raises(InvalidStatus) as ei:
        await connect(url(port), additional_headers={"Authorization": "Bearer nope"})
    assert ei.value.response.status_code == 401
    with pytest.raises(InvalidStatus):
        await connect(url(port, "?token=nope"))


async def test_auth_header_and_query_accepted(server):
    _, port = server
    async with await client(port) as ws:
        await call(ws, "echo", {"a": 1})
        assert (await recv(ws))["result"] == {"echo": {"a": 1}}
    async with connect(url(port, f"?token={TOKEN}")) as ws:
        await call(ws, "echo", [1, 2])
        assert (await recv(ws))["result"] == {"echo": [1, 2]}


async def test_response_shape(server):
    _, port = server
    async with await client(port) as ws:
        await call(ws, "echo", {}, id="abc")
        r = await recv(ws)
        assert r == {"jsonrpc": "2.0", "id": "abc", "result": {"echo": {}}}


async def test_concurrent_calls_overlap_and_complete_out_of_order(server):
    _, port = server
    async with await client(port) as ws:
        t0 = time.perf_counter()
        for i, s in enumerate([0.6, 0.4, 0.2, 0.1], start=1):
            await call(ws, "sleep", {"s": s}, id=i)
        order = [(await recv(ws))["id"] for _ in range(4)]
        elapsed = time.perf_counter() - t0
    assert order == [4, 3, 2, 1]  # shortest finishes first -> truly concurrent
    assert elapsed < 1.1  # serial would be 1.3 s


async def test_concurrent_connections(server):
    _, port = server
    async with await client(port) as a, await client(port) as b:
        await call(a, "sleep", {"s": 0.2}, id=1)
        await call(b, "echo", {"x": 1}, id=1)
        assert (await recv(b))["result"] == {"echo": {"x": 1}}
        assert (await recv(a))["result"] == {"slept": 0.2}


async def test_progress_notifications_precede_result(server):
    _, port = server
    async with await client(port) as ws:
        await call(ws, "progress", {"n": 5}, id=77)
        msgs = [await recv(ws) for _ in range(6)]
    notes, result = msgs[:5], msgs[5]
    assert [n["method"] for n in notes] == ["progress"] * 5
    assert all("id" not in n for n in notes)
    assert [n["params"] for n in notes] == [{"req": 77, "done": i, "total": 5} for i in range(1, 6)]
    assert result == {"jsonrpc": "2.0", "id": 77, "result": {"ok": True}}


async def test_error_shapes(server):
    _, port = server
    async with await client(port) as ws:
        await call(ws, "nope", id=1)
        r = await recv(ws)
        assert r["id"] == 1 and r["error"]["code"] == errors.METHOD_NOT_FOUND
        assert r["error"]["data"]["kind"] == "method_not_found"

        await call(ws, "invalid", id=2)
        r = await recv(ws)
        assert r["error"] == {
            "code": -32602,
            "message": "bad thing",
            "data": {"kind": "invalid_params", "detail": {"field": "x"}},
        }

        await call(ws, "custom", id=3)
        r = await recv(ws)
        assert r["error"]["code"] == -32010
        assert r["error"]["data"] == {"kind": "model_unavailable", "detail": {"models": ["m"]}}

        await call(ws, "boom", id=4)
        r = await recv(ws)
        assert r["error"]["code"] == errors.INTERNAL_ERROR
        assert "kaboom" in r["error"]["message"]
        assert "result" not in r

        # server still healthy after handler failures
        await call(ws, "echo", {"still": "ok"}, id=5)
        assert (await recv(ws))["result"] == {"echo": {"still": "ok"}}


async def test_protocol_errors(server):
    _, port = server
    async with await client(port) as ws:
        await ws.send("{not json")
        r = await recv(ws)
        assert r["id"] is None and r["error"]["code"] == errors.PARSE_ERROR

        await ws.send(json.dumps({"jsonrpc": "1.0", "id": 9, "method": "echo"}))
        r = await recv(ws)
        assert r["id"] == 9 and r["error"]["code"] == errors.INVALID_REQUEST

        await ws.send(json.dumps({"jsonrpc": "2.0", "id": 10}))
        assert (await recv(ws))["error"]["code"] == errors.INVALID_REQUEST

        await ws.send(json.dumps({"jsonrpc": "2.0", "id": 11, "method": "echo", "params": 5}))
        assert (await recv(ws))["error"]["code"] == errors.INVALID_REQUEST

        await ws.send("[]")
        assert (await recv(ws))["error"]["code"] == errors.INVALID_REQUEST


async def test_notification_gets_no_response(server):
    _, port = server
    async with await client(port) as ws:
        await ws.send(json.dumps({"jsonrpc": "2.0", "method": "echo", "params": {}}))
        await call(ws, "echo", {"after": 1}, id=1)
        r = await recv(ws)
        assert r["id"] == 1  # nothing was sent for the notification


async def test_batch(server):
    _, port = server
    async with await client(port) as ws:
        await ws.send(
            json.dumps(
                [
                    {"jsonrpc": "2.0", "id": 1, "method": "echo", "params": {"a": 1}},
                    {"jsonrpc": "2.0", "id": 2, "method": "nope"},
                    {"jsonrpc": "2.0", "method": "echo"},
                ]
            )
        )
        r = await recv(ws)
        assert isinstance(r, list) and len(r) == 2
        by_id = {x["id"]: x for x in r}
        assert by_id[1]["result"] == {"echo": {"a": 1}}
        assert by_id[2]["error"]["code"] == errors.METHOD_NOT_FOUND


async def test_cancel(server):
    _, port = server
    async with await client(port) as ws:
        await call(ws, "forever", id=1)
        await asyncio.sleep(0.1)
        await ws.send(json.dumps({"jsonrpc": "2.0", "method": "cancel", "params": {"req": 1}}))
        r = await recv(ws)
        assert r["id"] == 1 and r["error"]["code"] == errors.CANCELLED


async def test_graceful_shutdown_closes_clients(server):
    srv, port = server
    ws = await client(port)
    await call(ws, "forever", id=1)
    await asyncio.sleep(0.1)
    await asyncio.wait_for(srv.stop(), 5)
    with pytest.raises(Exception):  # noqa: B017  ConnectionClosed
        await asyncio.wait_for(ws.recv(), 2)
    with pytest.raises(OSError):
        await connect(url(port), additional_headers={"Authorization": f"Bearer {TOKEN}"})
    srv._server = None


# --------------------------------------------------------------------------- real service methods


@pytest_asyncio.fixture
async def service(tmp_path):
    from imagepicker_ai.service import WorkerService

    svc = WorkerService(
        models_dir=str(tmp_path / "models"), token=TOKEN, device="cpu", idle_unload_s=0
    )
    port = await svc.start("127.0.0.1", 0)
    yield svc, port
    await svc.stop()


async def test_system_info_and_models_list(service):
    _, port = service
    async with await client(port) as ws:
        await call(ws, "system.info", id=1)
        info = (await recv(ws))["result"]
        assert info["protocol"] == 1
        assert info["tier"] in ("T0", "T1", "T2", "T3")
        assert info["providers"][-1] == "CPUExecutionProvider"
        assert "hardware" in info and info["hardware"]["cpu_cores_logical"] >= 1
        assert set(info["steps"]) == {
            "phash",
            "quality",
            "faces",
            "identity",
            "embed",
            "aesthetic",
            "iqa",
            "scene",
        }
        assert info["profiles"]["fast"] == ["phash", "quality", "faces"]
        assert info["vram"]["budget_mb"] > 0

        await call(ws, "models.list", id=2)
        listed = (await recv(ws))["result"]
        models = listed["models"]
        ids = {m["id"] for m in models}
        assert {"yunet", "siglip2-base", "mediapipe-face-landmarker", "auraface"} <= ids
        by_id = {m["id"]: m for m in models}
        assert by_id["auraface"]["required_for"] == ["identity"]
        assert "scene" in by_id["siglip2-base-fp16"]["required_for"]
        assert by_id["nima-aesthetic"]["required_for"] == ["aesthetic"]
        assert set(listed["profiles"]) == {"fast", "standard"}
        assert "auraface" in listed["profiles"]["standard"]["models"]
        assert "auraface" not in listed["profiles"]["fast"]["models"]
        assert all(m["installed"] is False for m in models)

        await call(ws, "models.unload", {"id": "yunet"}, id=3)
        assert (await recv(ws))["result"]["unloaded"] == []
        await call(ws, "models.unload", {"id": "zzz"}, id=4)
        assert (await recv(ws))["error"]["code"] == errors.INVALID_PARAMS
        await call(ws, "models.ensure", {"id": "zzz"}, id=5)
        assert (await recv(ws))["error"]["code"] == errors.INVALID_PARAMS
        await call(ws, "models.ensure", {}, id=6)
        assert (await recv(ws))["error"]["code"] == errors.INVALID_PARAMS


async def test_analyze_batch_param_validation_and_missing_models(service, synth_files):
    _, port = service
    async with await client(port) as ws:
        await call(ws, "analyze.batch", {"items": []}, id=1)
        assert (await recv(ws))["error"]["code"] == errors.INVALID_PARAMS
        await call(
            ws,
            "analyze.batch",
            {"items": [{"photo_id": 1, "path": str(synth_files[0])}], "steps": ["embed"]},
            id=2,
        )
        r = await recv(ws)  # embed needs out_dir
        assert r["error"]["code"] == errors.INVALID_PARAMS
        await call(
            ws,
            "analyze.batch",
            {"items": [{"photo_id": 1, "path": str(synth_files[0])}], "steps": ["faces"]},
            id=3,
        )
        r = await recv(ws)  # yunet not installed and allow_download not set
        assert r["error"]["code"] == errors.MODEL_UNAVAILABLE
        assert r["error"]["data"]["detail"]["models"] == ["yunet"]


async def test_analyze_batch_classic_steps_over_rpc(service, synth_files, tmp_path):
    _, port = service
    items = [{"photo_id": i + 100, "path": str(p)} for i, p in enumerate(synth_files)]
    items.append({"photo_id": 999, "path": str(tmp_path / "missing.jpg")})
    async with await client(port) as ws:
        await call(
            ws,
            "analyze.batch",
            {
                "items": items,
                "steps": ["phash", "sharpness", "segment"],
                "analysis_size": 512,
                "out_dir": str(tmp_path / "out"),
            },
            id=1,
        )
        msgs = []
        while True:
            m = await recv(ws, 20)
            msgs.append(m)
            if m.get("id") == 1:
                break
    notes = [m for m in msgs if m.get("method") == "progress"]
    assert notes and notes[-1]["params"] == {"req": 1, "kind": "analyze", "done": 5, "total": 5}
    assert all(
        a["params"]["done"] <= b["params"]["done"] for a, b in zip(notes, notes[1:], strict=False)
    )
    res = msgs[-1]["result"]
    assert res["steps"] == ["phash", "quality"]
    assert res["skipped_steps"] == ["segment"]
    good = [i for i in res["items"] if "error" not in i]
    assert len(good) == 4
    assert all(len(i["phash"]) == 16 and 0 <= i["sharpness"] <= 1 for i in good)
    assert all(i["analysis_width"] <= 512 for i in good)
    bad = res["items"][-1]
    assert bad["photo_id"] == 999 and bad["error"]["kind"] == "decode_failed"
    assert (tmp_path / "out" / "100.analysis.json").exists()
