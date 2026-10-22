"""Cooperative cancellation for work that runs off the event loop.

The RPC layer cancels the asyncio task at once, so the client gets `-32800` immediately, but the
heavy work runs in a `ThreadPoolExecutor` (`inpaint.run`, `enhance.run`, `besttake.compose`,
`hair.compose`), where an asyncio cancel cannot reach: the thread kept computing to the end and
the next generation request queued behind it. A handler creates one `Cancel` per request and
passes `Cancel.check` (`None` when the caller does not care) into the work; the long loops call it
between steps and let `Cancelled` unwind the thread.
"""

from __future__ import annotations

import threading
from collections.abc import Callable

# What the compute code receives: a zero-argument function that raises `Cancelled` once the
# request has been cancelled, or `None` when nothing asked to cancel it.
Check = Callable[[], None]


class Cancelled(Exception):
    """The request was cancelled: stop computing and release the pool thread."""


class Cancel:
    """One request's cancellation flag. `set()` comes from the event loop, `check()` from the pool."""

    __slots__ = ("_event",)

    def __init__(self) -> None:
        self._event = threading.Event()

    def set(self) -> None:
        self._event.set()

    def is_set(self) -> bool:
        return self._event.is_set()

    def check(self) -> None:
        if self._event.is_set():
            raise Cancelled()
