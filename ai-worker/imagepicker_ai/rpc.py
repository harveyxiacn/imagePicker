"""JSON-RPC 2.0 over WebSocket (websockets asyncio server).

* Auth: `Authorization: Bearer <token>` header or `?token=<token>` query parameter, checked during the
  HTTP upgrade; a wrong/missing token gets HTTP 401 and no WebSocket is established.
* Requests run concurrently, one asyncio task each; handlers receive a `Ctx` to emit
  `progress` notifications `{"jsonrpc":"2.0","method":"progress","params":{"req":<id>, ...}}`.
* All outgoing frames of a connection go through one queue, so a request's progress
  notifications always precede its response.
* Notification (request without id) `cancel` / `$/cancel` with `{"req": <id>}` cancels an in-flight request
  (the request is answered with error -32800).
* Batch requests (JSON arrays) are supported.
"""

from __future__ import annotations

import asyncio
import hmac
import json
import logging
from collections.abc import Awaitable, Callable
from http import HTTPStatus
from typing import Any
from urllib.parse import parse_qs, urlsplit

from websockets.asyncio.server import Server, ServerConnection, serve
from websockets.exceptions import ConnectionClosed

from . import errors
from .errors import RpcError

log = logging.getLogger(__name__)

Handler = Callable[[Any, "Ctx"], Awaitable[Any]]


class Ctx:
    """Per-request context handed to handlers."""

    def __init__(self, req_id: Any, conn: _Conn):
        self.req_id = req_id
        self._conn = conn

    def progress(self, **fields: Any) -> None:
        self._conn.enqueue(
            {"jsonrpc": "2.0", "method": "progress", "params": {"req": self.req_id, **fields}}
        )

    def notify(self, method: str, params: Any) -> None:
        self._conn.enqueue({"jsonrpc": "2.0", "method": method, "params": params})


class _Conn:
    def __init__(self, ws: ServerConnection):
        self.ws = ws
        self.queue: asyncio.Queue[str | None] = asyncio.Queue()
        self.tasks: dict[Any, asyncio.Task[None]] = {}
        self.cancelled_ids: set[Any] = set()

    def enqueue(self, obj: Any) -> None:
        self.queue.put_nowait(
            json.dumps(obj, separators=(",", ":"), allow_nan=False, default=_json_default)
        )


def _json_default(o: Any) -> Any:
    import numpy as np

    if isinstance(o, np.generic):
        return o.item()
    if isinstance(o, np.ndarray):
        return o.tolist()
    if hasattr(o, "__fspath__"):
        return str(o)
    raise TypeError(f"not JSON serializable: {type(o).__name__}")


def _err_response(req_id: Any, err: RpcError) -> dict[str, Any]:
    return {"jsonrpc": "2.0", "id": req_id, "error": err.to_obj()}


class RpcServer:
    def __init__(self, handlers: dict[str, Handler], token: str | None, max_message_mb: int = 64):
        self.handlers = handlers
        self.token = token
        self.max_size = max_message_mb * 1024 * 1024
        self._server: Server | None = None
        self._conns: set[_Conn] = set()
        self.port: int | None = None

    # ------------------------------------------------------------------ lifecycle
    async def start(self, host: str = "127.0.0.1", port: int = 0) -> int:
        self._server = await serve(
            self._handle,
            host,
            port,
            process_request=self._authenticate,
            max_size=self.max_size,
            ping_interval=20,
            ping_timeout=60,
        )
        sock = next(iter(self._server.sockets))
        self.port = sock.getsockname()[1]
        return self.port

    async def stop(self) -> None:
        """Graceful shutdown: stop accepting, cancel in-flight requests, close connections."""
        if self._server is None:
            return
        self._server.close()  # stop listening and close all connections (1001)
        for c in list(self._conns):
            for t in list(c.tasks.values()):
                t.cancel()
        await self._server.wait_closed()
        self._server = None

    # ------------------------------------------------------------------ auth
    def _authenticate(self, connection: ServerConnection, request: Any) -> Any:
        if self.token is None:
            return None
        supplied = None
        auth = request.headers.get("Authorization", "")
        if auth.lower().startswith("bearer "):
            supplied = auth[7:].strip()
        if supplied is None:
            q = parse_qs(urlsplit(request.path).query)
            supplied = (q.get("token") or [None])[0]
        if supplied is None or not hmac.compare_digest(supplied.encode(), self.token.encode()):
            log.warning("rejected connection: bad or missing token")
            return connection.respond(HTTPStatus.UNAUTHORIZED, "unauthorized\n")
        return None

    # ------------------------------------------------------------------ connection loop
    async def _handle(self, ws: ServerConnection) -> None:
        conn = _Conn(ws)
        self._conns.add(conn)
        sender = asyncio.create_task(self._sender(conn))
        try:
            async for raw in ws:
                await self._on_message(conn, raw)
        except ConnectionClosed:
            pass
        finally:
            self._conns.discard(conn)
            for t in list(conn.tasks.values()):
                t.cancel()
            if conn.tasks:
                await asyncio.gather(*conn.tasks.values(), return_exceptions=True)
            conn.queue.put_nowait(None)
            await asyncio.gather(sender, return_exceptions=True)

    async def _sender(self, conn: _Conn) -> None:
        try:
            while (msg := await conn.queue.get()) is not None:
                await conn.ws.send(msg)
        except ConnectionClosed:
            pass

    async def _on_message(self, conn: _Conn, raw: str | bytes) -> None:
        try:
            msg = json.loads(raw)
        except (ValueError, UnicodeDecodeError):
            conn.enqueue(_err_response(None, RpcError(errors.PARSE_ERROR, "parse error")))
            return
        if isinstance(msg, list):
            if not msg:
                conn.enqueue(_err_response(None, RpcError(errors.INVALID_REQUEST, "empty batch")))
                return
            task = asyncio.create_task(self._run_batch(conn, msg))
            key = object()
            conn.tasks[key] = task
            task.add_done_callback(lambda _t, k=key: conn.tasks.pop(k, None))
            return
        self._dispatch(conn, msg, respond=lambda r: conn.enqueue(r))

    def _dispatch(
        self, conn: _Conn, msg: Any, respond: Callable[[Any], None]
    ) -> asyncio.Task[None] | None:
        if (
            not isinstance(msg, dict)
            or msg.get("jsonrpc") != "2.0"
            or not isinstance(msg.get("method"), str)
        ):
            rid = (
                msg.get("id")
                if isinstance(msg, dict) and isinstance(msg.get("id"), (int, str))
                else None
            )
            respond(_err_response(rid, RpcError(errors.INVALID_REQUEST, "invalid request")))
            return None
        method: str = msg["method"]
        params = msg.get("params")
        has_id = "id" in msg
        rid = msg.get("id")
        if has_id and (
            isinstance(rid, bool) or not isinstance(rid, (int, str)) and rid is not None
        ):
            respond(_err_response(None, RpcError(errors.INVALID_REQUEST, "invalid id")))
            return None
        if params is not None and not isinstance(params, (dict, list)):
            if has_id:
                respond(
                    _err_response(
                        rid, RpcError(errors.INVALID_REQUEST, "params must be object or array")
                    )
                )
            return None

        if method in ("cancel", "$/cancel"):
            target = params.get("req") if isinstance(params, dict) else None
            t = conn.tasks.get(target)
            if t is not None:
                conn.cancelled_ids.add(target)
                t.cancel()
            if has_id:
                respond({"jsonrpc": "2.0", "id": rid, "result": {"cancelled": t is not None}})
            return None

        handler = self.handlers.get(method)
        if handler is None:
            if has_id:
                respond(
                    _err_response(
                        rid,
                        RpcError(
                            errors.METHOD_NOT_FOUND,
                            f"method not found: {method}",
                            "method_not_found",
                        ),
                    )
                )
            return None

        task = asyncio.create_task(self._call(conn, handler, rid, params, has_id, respond))
        if has_id:
            if rid in conn.tasks:
                task.cancel()
                respond(
                    _err_response(
                        rid, RpcError(errors.INVALID_REQUEST, f"duplicate in-flight id {rid!r}")
                    )
                )
                return None
            conn.tasks[rid] = task
            task.add_done_callback(lambda _t, k=rid: conn.tasks.pop(k, None))
        return task

    async def _call(
        self,
        conn: _Conn,
        handler: Handler,
        rid: Any,
        params: Any,
        has_id: bool,
        respond: Callable[[Any], None],
    ) -> None:
        ctx = Ctx(rid, conn)
        try:
            result = await handler(params if params is not None else {}, ctx)
            if has_id:
                respond({"jsonrpc": "2.0", "id": rid, "result": result})
        except asyncio.CancelledError:
            if has_id and rid in conn.cancelled_ids:
                conn.cancelled_ids.discard(rid)
                respond(
                    _err_response(rid, RpcError(errors.CANCELLED, "request cancelled", "cancelled"))
                )
                return
            raise
        except RpcError as e:
            if has_id:
                respond(_err_response(rid, e))
        except Exception as e:  # noqa: BLE001
            log.exception("handler error")
            if has_id:
                respond(
                    _err_response(
                        rid, RpcError(errors.INTERNAL_ERROR, f"{type(e).__name__}: {e}", "internal")
                    )
                )

    async def _run_batch(self, conn: _Conn, msgs: list[Any]) -> None:
        responses: list[Any] = []
        tasks: list[asyncio.Task[None]] = []
        for m in msgs:
            t = self._dispatch(conn, m, respond=responses.append)
            if t is not None:
                tasks.append(t)
        # Note: individual calls respond via `responses.append`, so wait for them all
        if tasks:
            await asyncio.gather(*tasks, return_exceptions=True)
        if responses:
            conn.enqueue(responses)
