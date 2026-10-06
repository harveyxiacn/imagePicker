"""RPC method implementations + worker lifecycle (`WorkerService`)."""

from __future__ import annotations

import asyncio
import logging
import os
import threading
import time
from concurrent.futures import ThreadPoolExecutor
from typing import Any

from . import PROTOCOL_VERSION, __version__
from . import hw as hwmod
from .errors import DownloadFailed, InvalidParams, RpcError
from .models.download import Cancelled
from .models.manager import ModelManager
from .models.registry import Registry
from .paths import resolve_models_dir
from .pipeline import Analyzer
from .rpc import Ctx, RpcServer
from .steps import PROFILES, STEPS

log = logging.getLogger(__name__)


class WorkerService:
    def __init__(
        self,
        models_dir: str | None = None,
        token: str | None = None,
        device: str = "auto",
        registry: Registry | None = None,
        idle_unload_s: float = 600.0,
        hardware: hwmod.HardwareInfo | None = None,
        decode_workers: int | None = None,
    ):
        self.hw = hardware or hwmod.detect(device)
        self.models_dir = resolve_models_dir(models_dir)
        self.registry = registry or Registry.load()
        self.manager = ModelManager(self.models_dir, self.registry, self.hw)
        self.analyzer = Analyzer(self.manager, self.hw, decode_workers=decode_workers)
        self.io_pool = ThreadPoolExecutor(2, thread_name_prefix="io")
        self.idle_unload_s = idle_unload_s
        self.started_at = time.time()
        self.stop_event = asyncio.Event()
        self._ensure_locks: dict[str, asyncio.Lock] = {}
        self.server = RpcServer(self._handlers(), token)
        self._sweeper: asyncio.Task[None] | None = None

    def _handlers(self) -> dict[str, Any]:
        return {
            "system.info": self.system_info,
            "system.ping": self.ping,
            "system.shutdown": self.shutdown_rpc,
            "models.list": self.models_list,
            "models.ensure": self.models_ensure,
            "models.unload": self.models_unload,
            "analyze.batch": self.analyze_batch,
        }

    # ------------------------------------------------------------------ lifecycle
    async def start(self, host: str = "127.0.0.1", port: int = 0) -> int:
        p = await self.server.start(host, port)
        if self.idle_unload_s > 0:
            self._sweeper = asyncio.create_task(self._sweep_loop())
        return p

    async def stop(self) -> None:
        if self._sweeper:
            self._sweeper.cancel()
        await self.server.stop()
        self.analyzer.shutdown()
        self.manager.unload()
        self.io_pool.shutdown(wait=False, cancel_futures=True)

    async def _sweep_loop(self) -> None:
        while True:
            await asyncio.sleep(min(60.0, max(1.0, self.idle_unload_s / 4)))
            gone = self.manager.sweep_idle(self.idle_unload_s)
            if gone:
                log.info("idle-unloaded: %s", gone)

    # ------------------------------------------------------------------ methods
    async def ping(self, _params: Any, _ctx: Ctx) -> dict[str, Any]:
        return {"pong": True, "time": time.time()}

    async def shutdown_rpc(self, _params: Any, _ctx: Ctx) -> dict[str, Any]:
        asyncio.get_running_loop().call_later(0.05, self.stop_event.set)
        return {"shutting_down": True}

    async def system_info(self, _params: Any, _ctx: Ctx) -> dict[str, Any]:
        loop = asyncio.get_running_loop()
        live = await loop.run_in_executor(self.io_pool, hwmod.nvidia_memory_usage)
        from .steps.faces import mediapipe_available

        return {
            "version": __version__,
            "protocol": PROTOCOL_VERSION,
            "pid": os.getpid(),
            "uptime_s": round(time.time() - self.started_at, 1),
            "hardware": self.hw.to_dict(),
            "tier": self.hw.tier,
            "providers": self.hw.providers,
            "models_dir": str(self.models_dir),
            "loaded_models": self.manager.loaded(),
            "vram": {
                "budget_mb": self.manager.budget_mb,
                "managed_used_mb": self.manager.used_mb(),
                "gpu": live,
            },
            "steps": list(STEPS),
            "profiles": {k: list(v) for k, v in PROFILES.items()},
            "features": {"mediapipe": mediapipe_available(), "heif": _heif_ok()},
        }

    async def models_list(self, _params: Any, _ctx: Ctx) -> dict[str, Any]:
        profiles = {
            name: {"steps": list(steps), "models": self.analyzer.models_for_steps(list(steps))}
            for name, steps in PROFILES.items()
        }
        return {
            "models": self.manager.list_models(),
            "models_dir": str(self.models_dir),
            "profiles": profiles,
        }

    async def models_ensure(self, params: Any, ctx: Ctx) -> dict[str, Any]:
        ids = _model_ids(params)
        for i in ids:
            if i not in self.registry:
                raise InvalidParams(f"unknown model id {i!r}")
        loop = asyncio.get_running_loop()
        results = []
        for mid in ids:
            lock = self._ensure_locks.setdefault(mid, asyncio.Lock())
            cancel = threading.Event()
            async with lock:

                def on_progress(e: dict[str, Any], _mid: str = mid) -> None:
                    fields = {**e, "kind": "model.download", "done": e["bytes"]}
                    loop.call_soon_threadsafe(lambda f=fields: ctx.progress(**f))

                try:
                    path = await loop.run_in_executor(
                        self.io_pool, lambda m=mid, c=cancel: self.manager.ensure(m, on_progress, c)
                    )
                except asyncio.CancelledError:
                    cancel.set()
                    raise
                except Cancelled:
                    raise asyncio.CancelledError from None
                except RpcError:
                    raise
                except Exception as e:  # noqa: BLE001
                    raise DownloadFailed(f"{mid}: {e}") from e
            spec = self.registry.get(mid)
            results.append(
                {
                    "id": mid,
                    "installed": True,
                    "path": str(path),
                    "size_bytes": self.manager.downloader.installed_size(spec),
                }
            )
        return {"models": results}

    async def models_unload(self, params: Any, _ctx: Ctx) -> dict[str, Any]:
        mid = params.get("id") if isinstance(params, dict) else None
        if mid is not None and mid not in self.registry:
            raise InvalidParams(f"unknown model id {mid!r}")
        gone = self.manager.unload(mid)
        return {"unloaded": gone, "loaded": self.manager.loaded()}

    async def analyze_batch(self, params: Any, ctx: Ctx) -> dict[str, Any]:
        return await self.analyzer.analyze_batch(params, lambda p: ctx.progress(**p))


def _model_ids(params: Any) -> list[str]:
    if not isinstance(params, dict):
        raise InvalidParams("params must be an object")
    if "ids" in params:
        ids = params["ids"]
    elif "id" in params:
        ids = [params["id"]]
    else:
        raise InvalidParams("`id` or `ids` required")
    if not isinstance(ids, list) or not ids or not all(isinstance(i, str) for i in ids):
        raise InvalidParams("ids must be a non-empty array of strings")
    return ids


def _heif_ok() -> bool:
    try:
        import pillow_heif  # noqa: F401

        return True
    except Exception:  # noqa: BLE001
        return False
