"""Crash records for the worker (gap P1-13).

The core passes ``IMAGEPICKER_CRASH_DIR`` (``<data>/crashes``) when it starts the worker. Unhandled
exceptions (main thread, other threads, asyncio tasks) are written there as ``worker-crash-*.txt``;
native faults (segfaults in onnxruntime, torch, ...) are dumped by :mod:`faulthandler` into
``worker-native-*.log``, which is deleted again on a clean exit when nothing was written.
"""

from __future__ import annotations

import atexit
import faulthandler
import os
import platform
import sys
import threading
import time
import traceback
from pathlib import Path
from types import TracebackType
from typing import IO, Any

ENV_CRASH_DIR = "IMAGEPICKER_CRASH_DIR"
KEEP = 30

_native_file: IO[str] | None = None
_installed_dir: Path | None = None


def _stamp() -> str:
    return time.strftime("%Y%m%d-%H%M%S", time.gmtime())


def _version() -> str:
    from . import __version__

    return __version__


def write_report(
    crash_dir: Path,
    kind: str,
    exc: BaseException | None,
    message: str = "",
    thread: str | None = None,
) -> Path | None:
    """Writes ``worker-crash-<utc>-<pid>-<n>.txt``; never raises."""
    try:
        crash_dir.mkdir(parents=True, exist_ok=True)
        n = 0
        while True:
            path = crash_dir / f"worker-crash-{_stamp()}-{os.getpid()}-{n}.txt"
            if not path.exists():
                break
            n += 1
        lines = [
            "imagePicker AI worker crash record",
            f"kind: {kind}",
            f"time: {time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime())}",
            f"version: {_version()}",
            f"os: {platform.system().lower()} {platform.machine()}",
            f"python: {platform.python_version()}",
            f"pid: {os.getpid()}",
            f"thread: {thread or threading.current_thread().name}",
        ]
        if exc is not None:
            lines.append(f"message: {type(exc).__name__}: {exc}")
            lines.append("")
            lines.append("traceback:")
            lines.extend(
                "".join(traceback.format_exception(type(exc), exc, exc.__traceback__)).splitlines()
            )
        else:
            lines.append(f"message: {message}")
        path.write_text("\n".join(lines) + "\n", encoding="utf-8")
        _trim(crash_dir)
        return path
    except Exception:  # noqa: BLE001 - the crash handler must not crash
        return None


def _trim(crash_dir: Path) -> None:
    files = sorted(
        (p for p in crash_dir.glob("worker-*") if p.is_file()),
        key=lambda p: p.stat().st_mtime,
        reverse=True,
    )
    for p in files[KEEP:]:
        p.unlink(missing_ok=True)


def _close_native() -> None:
    global _native_file
    f, _native_file = _native_file, None
    if f is None:
        return
    try:
        faulthandler.disable()
        path = Path(f.name)
        f.close()
        if path.stat().st_size == 0:
            path.unlink(missing_ok=True)
    except OSError:
        pass


def install(crash_dir: str | os.PathLike[str] | None = None) -> Path | None:
    """Starts capturing crashes into ``crash_dir`` (default: ``$IMAGEPICKER_CRASH_DIR``).

    Returns the directory, or None when none is configured.
    """
    global _native_file, _installed_dir
    raw = crash_dir or os.environ.get(ENV_CRASH_DIR)
    if not raw:
        return None
    d = Path(raw)
    try:
        d.mkdir(parents=True, exist_ok=True)
        # dumps of earlier runs that never faulted are empty: drop them (a killed worker cannot)
        for old in d.glob("worker-native-*.log"):
            try:
                if old.stat().st_size == 0:
                    old.unlink()
            except OSError:
                pass
        native = d / f"worker-native-{_stamp()}-{os.getpid()}.log"
        _native_file = open(native, "w", encoding="utf-8")  # noqa: SIM115 - lives for the process
        faulthandler.enable(file=_native_file, all_threads=True)
        atexit.register(_close_native)
    except OSError:
        pass
    _installed_dir = d

    prev_hook = sys.excepthook

    def excepthook(
        etype: type[BaseException], exc: BaseException, tb: TracebackType | None
    ) -> None:
        if not issubclass(etype, KeyboardInterrupt):
            write_report(d, "python-unhandled", exc)
        prev_hook(etype, exc, tb)

    sys.excepthook = excepthook

    prev_thread_hook = threading.excepthook

    def thread_hook(args: threading.ExceptHookArgs) -> None:
        if args.exc_value is not None and not issubclass(args.exc_type, SystemExit):
            write_report(
                d,
                "python-thread",
                args.exc_value,
                thread=args.thread.name if args.thread else None,
            )
        prev_thread_hook(args)

    threading.excepthook = thread_hook
    return d


def loop_exception_handler(loop: Any, context: dict[str, Any]) -> None:
    """``loop.set_exception_handler`` target: records errors nobody awaited, then logs them."""
    if _installed_dir is not None:
        exc = context.get("exception")
        if isinstance(exc, BaseException):
            write_report(_installed_dir, "python-asyncio", exc)
        else:
            write_report(
                _installed_dir, "python-asyncio", None, message=str(context.get("message", ""))
            )
    loop.default_exception_handler(context)
