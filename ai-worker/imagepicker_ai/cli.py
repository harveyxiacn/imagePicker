"""`imagepicker-ai serve --host 127.0.0.1 --port 0 --token T`

stdout carries exactly one line at startup: {"event":"ready","port":N}. Logs go to stderr.
Env fallbacks: IP_WORKER_ADDR (host:port), IP_WORKER_TOKEN, IMAGEPICKER_MODELS_DIR.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import logging
import os
import signal
import sys

import psutil


def _parse_addr(addr: str) -> tuple[str, int]:
    host, _, port = addr.rpartition(":")
    return (host.strip("[]") or "127.0.0.1"), int(port or 0)


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(prog="imagepicker-ai")
    sub = p.add_subparsers(dest="cmd", required=True)
    s = sub.add_parser("serve", help="run the JSON-RPC WebSocket worker")
    s.add_argument("--host", default=None, help="bind address (default 127.0.0.1)")
    s.add_argument(
        "--port", type=int, default=None, help="0 = pick a free port (reported on stdout)"
    )
    s.add_argument("--token", default=None, help="auth token (or env IP_WORKER_TOKEN)")
    s.add_argument(
        "--no-auth", action="store_true", help="disable token auth (loopback testing only)"
    )
    s.add_argument("--models-dir", default=None)
    s.add_argument(
        "--device", choices=["auto", "cpu"], default="auto", help="'cpu' disables GPU providers"
    )
    s.add_argument(
        "--idle-unload-s",
        type=float,
        default=600.0,
        help="unload idle non-resident models (0 = never)",
    )
    s.add_argument("--parent-pid", type=int, default=None, help="exit when this process disappears")
    s.add_argument("--log-level", default="INFO")
    return p


async def _serve(args: argparse.Namespace) -> int:
    from .service import WorkerService

    env_host, env_port = (
        _parse_addr(os.environ["IP_WORKER_ADDR"])
        if os.environ.get("IP_WORKER_ADDR")
        else (None, None)
    )
    host = args.host or env_host or "127.0.0.1"
    port = args.port if args.port is not None else (env_port if env_port is not None else 0)
    token = args.token or os.environ.get("IP_WORKER_TOKEN")
    if not token and not args.no_auth:
        print(
            "error: a token is required (--token / IP_WORKER_TOKEN), or pass --no-auth",
            file=sys.stderr,
        )
        return 2

    svc = WorkerService(
        models_dir=args.models_dir,
        token=None if args.no_auth else token,
        device=args.device,
        idle_unload_s=args.idle_unload_s,
    )
    bound = await svc.start(host, port)
    sys.stdout.write(json.dumps({"event": "ready", "port": bound}) + "\n")
    sys.stdout.flush()
    logging.getLogger("imagepicker_ai").info(
        "worker ready on %s:%d tier=%s providers=%s", host, bound, svc.hw.tier, svc.hw.providers
    )

    loop = asyncio.get_running_loop()
    for sig in (signal.SIGINT, signal.SIGTERM):
        try:
            loop.add_signal_handler(sig, svc.stop_event.set)
        except (NotImplementedError, RuntimeError):  # Windows
            signal.signal(sig, lambda *_: loop.call_soon_threadsafe(svc.stop_event.set))
    if hasattr(signal, "SIGBREAK"):
        signal.signal(signal.SIGBREAK, lambda *_: loop.call_soon_threadsafe(svc.stop_event.set))

    async def watch_parent() -> None:
        while psutil.pid_exists(args.parent_pid):  # noqa: ASYNC110 - polling a foreign pid
            await asyncio.sleep(2)
        logging.getLogger("imagepicker_ai").warning(
            "parent %s gone, shutting down", args.parent_pid
        )
        svc.stop_event.set()

    watcher = asyncio.create_task(watch_parent()) if args.parent_pid else None
    await svc.stop_event.wait()
    if watcher:
        watcher.cancel()
    await svc.stop()
    return 0


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    logging.basicConfig(
        level=args.log_level.upper(),
        stream=sys.stderr,
        format="%(asctime)s %(levelname)s %(name)s: %(message)s",
    )
    if args.cmd == "serve":
        try:
            return asyncio.run(_serve(args))
        except KeyboardInterrupt:
            return 0
    return 1
