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
from .beauty import BeautyPreparer
from .besttake import BestTake
from .enhance import Enhancer
from .errors import DownloadFailed, InvalidParams, RpcError
from .inpaint import Inpainter
from .llm import Assistant
from .masks import TARGETS as MASK_TARGETS
from .masks import MaskGenerator
from .models.download import Cancelled
from .models.manager import ModelManager
from .models.registry import Registry
from .paths import resolve_models_dir
from .pipeline import Analyzer
from .rpc import Ctx, RpcServer
from .runtime import GenRuntime
from .steps import PROFILES, STEPS
from .steps.face_search import faces_embed

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
        self.masks = MaskGenerator(self.manager, self.hw)
        self.beauty = BeautyPreparer(self.masks)
        self.gen = GenRuntime(self.manager, self.hw)
        self.besttake = BestTake(self.masks)
        self.inpaint = Inpainter(self.gen)
        self.enhance = Enhancer(self.gen, self.masks)
        self.assistant = Assistant(self.manager, self.hw)
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
            "models.delete": self.models_delete,
            "analyze.batch": self.analyze_batch,
            "mask.generate": self.mask_generate,
            "beauty.prepare": self.beauty_prepare,
            "faces.embed": self.faces_embed,
            "besttake.compose": self.besttake_compose,
            "inpaint.run": self.inpaint_run,
            "enhance.run": self.enhance_run,
            "llm.plan": self.llm_plan,
            "vlm.suggest": self.vlm_suggest,
            "vlm.describe": self.vlm_describe,
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
        self.beauty.shutdown()
        self.besttake.shutdown()
        self.enhance.shutdown()
        self.gen.shutdown()
        self.assistant.shutdown()
        self.masks.shutdown()
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

        assistant = self.assistant.status()
        assistant_status = {
            "llm_available": assistant["llm_available"],
            "vlm_available": assistant["vlm_available"],
            "assistant": assistant,
        }
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
            "mask_targets": list(MASK_TARGETS),
            "profiles": {k: list(v) for k, v in PROFILES.items()},
            "features": {"mediapipe": mediapipe_available(), "heif": _heif_ok()},
            **assistant_status,
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
            # what `models.ensure` must fetch on this machine for each mask target
            "beauty_models": self.beauty.required_models(),
            # M5: what `models.ensure` must fetch for each generative method on this machine
            "besttake_models": self.besttake.all_models(),
            "inpaint_models": {m: self.inpaint.required_models(m) for m in ("lama", "sdxl")},
            "enhance_models": {
                "denoise": self.enhance.required_models("denoise"),
                "face_restore": self.enhance.required_models("face_restore"),
                "upscale": {str(sc): self.enhance.required_models("upscale", sc) for sc in (2, 4)},
            },
            "mask_models": {
                t: self.masks.required_models(t) + self.masks.optional_models(t)
                for t in MASK_TARGETS
            },
            # M6: the LLM / VLM `models.ensure` has to fetch on this machine ([] = tier too low)
            "assistant_models": self.assistant.default_models(),
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

    async def models_delete(self, params: Any, _ctx: Ctx) -> dict[str, Any]:
        """Remove a downloaded model's files (unloading it first when loaded)."""
        mid = params.get("id") if isinstance(params, dict) else None
        if not isinstance(mid, str) or mid not in self.registry:
            raise InvalidParams(f"unknown model id {mid!r}")
        spec = self.registry.get(mid)
        loop = asyncio.get_running_loop()

        def work() -> int:
            freed = self.manager.downloader.installed_size(spec)
            self.manager.delete(mid)
            return freed

        freed = await loop.run_in_executor(self.io_pool, work)
        return {"deleted": True, "freed_bytes": freed}

    async def analyze_batch(self, params: Any, ctx: Ctx) -> dict[str, Any]:
        return await self.analyzer.analyze_batch(params, lambda p: ctx.progress(**p))

    async def mask_generate(self, params: Any, ctx: Ctx) -> dict[str, Any]:
        return await self.masks.generate(params, lambda p: ctx.progress(**p))

    async def beauty_prepare(self, params: Any, ctx: Ctx) -> dict[str, Any]:
        return await self.beauty.prepare(params, lambda p: ctx.progress(**p))

    async def besttake_compose(self, params: Any, ctx: Ctx) -> dict[str, Any]:
        return await self.besttake.compose(params, lambda p: ctx.progress(**p))

    async def inpaint_run(self, params: Any, ctx: Ctx) -> dict[str, Any]:
        return await self.inpaint.run(params, lambda p: ctx.progress(**p))

    async def enhance_run(self, params: Any, ctx: Ctx) -> dict[str, Any]:
        return await self.enhance.run(params, lambda p: ctx.progress(**p))

    async def llm_plan(self, params: Any, ctx: Ctx) -> dict[str, Any]:
        return await self.assistant.plan(params, lambda p: ctx.progress(**p))

    async def vlm_suggest(self, params: Any, ctx: Ctx) -> dict[str, Any]:
        return await self.assistant.suggest(params, lambda p: ctx.progress(**p))

    async def vlm_describe(self, params: Any, ctx: Ctx) -> dict[str, Any]:
        return await self.assistant.describe(params, lambda p: ctx.progress(**p))

    async def faces_embed(self, params: Any, _ctx: Ctx) -> dict[str, Any]:
        return await faces_embed(self.analyzer, params, self.analyzer.decode_pool)


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
