"""Crash records of the worker (gap P1-13)."""

from __future__ import annotations

import asyncio
import faulthandler
import sys
import threading
from pathlib import Path

import pytest

from imagepicker_ai import crash


@pytest.fixture
def hooks():
    """Restores the process-wide hooks that `crash.install` replaces."""
    saved = (sys.excepthook, threading.excepthook, crash._installed_dir)
    yield
    crash._close_native()
    sys.excepthook, threading.excepthook, crash._installed_dir = saved


def reports(d: Path) -> list[Path]:
    return sorted(d.glob("worker-crash-*.txt"))


def test_install_without_a_directory_is_a_noop(monkeypatch, hooks):
    monkeypatch.delenv(crash.ENV_CRASH_DIR, raising=False)
    before = sys.excepthook
    assert crash.install() is None
    assert sys.excepthook is before


def test_unhandled_exception_is_recorded(tmp_path, hooks):
    d = tmp_path / "crashes"
    assert crash.install(d) == d
    assert faulthandler.is_enabled()
    try:
        raise ValueError("boom 7f3a")
    except ValueError as e:
        # what the interpreter calls for an exception nobody caught (prev hook prints to stderr)
        sys.excepthook(type(e), e, e.__traceback__)
    (report,) = reports(d)
    text = report.read_text(encoding="utf-8")
    for needle in (
        "kind: python-unhandled",
        "time: 20",
        "version: ",
        "os: ",
        "python: 3.",
        "thread: ",
        "message: ValueError: boom 7f3a",
        "Traceback (most recent call last)",
    ):
        assert needle in text, needle
    # the native-fault dump exists and stays empty until a fault happens
    (native,) = d.glob("worker-native-*.log")
    assert native.stat().st_size == 0


def test_thread_exception_is_recorded(tmp_path, hooks):
    d = tmp_path / "crashes"
    threading.excepthook = lambda _args: None  # the hook install() chains to; keep pytest quiet
    crash.install(d)

    def run():
        raise RuntimeError("thread boom 91c2")

    t = threading.Thread(target=run, name="worker-thread-x")
    t.start()
    t.join()
    (report,) = reports(d)
    text = report.read_text(encoding="utf-8")
    assert "kind: python-thread" in text
    assert "thread: worker-thread-x" in text
    assert "RuntimeError: thread boom 91c2" in text


def test_asyncio_exception_is_recorded(tmp_path, hooks):
    d = tmp_path / "crashes"
    crash.install(d)

    async def main():
        loop = asyncio.get_running_loop()
        loop.set_exception_handler(crash.loop_exception_handler)
        loop.call_exception_handler({"message": "x", "exception": KeyError("asyncio boom 5d")})

    asyncio.run(main())
    (report,) = reports(d)
    assert "kind: python-asyncio" in report.read_text(encoding="utf-8")


def test_reports_are_trimmed(tmp_path):
    for i in range(crash.KEEP + 5):
        assert crash.write_report(tmp_path, "python-unhandled", None, message=f"m{i}")
    assert len(list(tmp_path.glob("worker-*"))) <= crash.KEEP


def test_clean_exit_removes_the_empty_native_dump(tmp_path, hooks):
    crash.install(tmp_path)
    assert list(tmp_path.glob("worker-native-*.log"))
    crash._close_native()
    assert not list(tmp_path.glob("worker-native-*.log"))
