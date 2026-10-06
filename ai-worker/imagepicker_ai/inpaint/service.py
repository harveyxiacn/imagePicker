"""`inpaint.run` (docs/api-contract-m5.md section B)."""

from __future__ import annotations

import asyncio
import hashlib
import logging
from pathlib import Path
from typing import Any

import numpy as np

from ..errors import InvalidParams, ModelUnavailable, OutOfMemory
from ..imgio import (
    load_rgb,
    norm_rect,
    parse_out_dir,
    parse_photo,
    read_mask,
    safe_name,
    write_rgba_png,
)
from ..masks.generator import ProgressFn
from ..models.manager import ModelManager
from ..runtime import MIN_TILE, GenRuntime, OrtHandle
from ..steps.embed import _is_oom
from . import sdxl
from .core import FillEngine, inpaint_arrays

log = logging.getLogger(__name__)

LAMA = "lama-big-fp32"
MODELS = {"lama": LAMA, "sdxl": sdxl.MODEL_ID}


class Inpainter:
    def __init__(self, runtime: GenRuntime):
        self.rt = runtime
        self.manager: ModelManager = runtime.manager

    def required_models(self, model: str = "lama") -> list[str]:
        return [MODELS[model]]

    # ------------------------------------------------------------------ engines
    def _lama_engine(self) -> tuple[FillEngine, Any]:
        def run(h: OrtHandle, _tile: int, img: np.ndarray, mask: np.ndarray) -> np.ndarray:
            x = np.ascontiguousarray(img.transpose(2, 0, 1)[None].astype(np.float32) * (1 / 255.0))
            m = np.ascontiguousarray(mask[None, None].astype(np.float32))
            y = h.run({"image": x, "mask": m})[0]  # 0..255
            return np.clip(np.rint(y.transpose(1, 2, 0)), 0, 255).astype(np.uint8)

        def fn(img: np.ndarray, mask: np.ndarray) -> np.ndarray:
            # fixed 512 network: tile halving cannot help, so an OOM goes straight to the CPU
            return self.rt.run_with_backoff(LAMA, lambda h, t: run(h, t, img, mask), MIN_TILE)

        return FillEngine(512, fn), None

    def _sdxl_engine(self) -> tuple[FillEngine, Any]:
        spec = self.manager.registry.get(sdxl.MODEL_ID)
        budget = self.manager.budget_mb
        offload = budget < spec.vram_mb + 1500
        state = {"offload": offload}

        def loader_for(off: bool):
            return lambda s, path: sdxl.SdxlHandle(Path(path).parent, off)

        def fn(img: np.ndarray, mask: np.ndarray) -> np.ndarray:
            for _attempt in range(2):
                off = state["offload"]
                cost = sdxl.OFFLOAD_COST_MB if off else None
                try:
                    with self.manager.lease(sdxl.MODEL_ID, loader_for(off), cost) as h:
                        return h.fill(img, mask)
                except Exception as e:  # noqa: BLE001
                    if not _is_oom(e):
                        raise
                    self.manager.unload(sdxl.MODEL_ID)
                    if state["offload"]:
                        raise OutOfMemory(str(e)) from e
                    log.warning("SDXL OOM; rebuilding the pipeline with CPU offload")
                    state["offload"] = True
            raise OutOfMemory("sdxl inpainting out of memory")

        return FillEngine(sdxl.SIZE, fn, min_context=128), None

    # ------------------------------------------------------------------ run
    def _run(self, req: dict[str, Any]) -> dict[str, Any]:
        req["out_dir"].mkdir(parents=True, exist_ok=True)
        img = load_rgb(req["photo"])
        h, w = img.shape[:2]
        mask = read_mask(req["mask"], w, h)
        engine = self._sdxl_engine()[0] if req["model"] == "sdxl" else self._lama_engine()[0]
        out = inpaint_arrays(img, mask, engine)
        if out is None:
            return {"patch": None, "reason": "empty_mask"}
        digest = hashlib.sha1(Path(req["mask"]).read_bytes()).hexdigest()[:8]  # noqa: S324
        path = req["out_dir"] / f"inpaint_{safe_name(req['photo'].photo_id)}_{digest}.png"
        write_rgba_png(path, out.patch[..., :3], out.patch[..., 3])
        return {
            "patch": str(path),
            "rect": norm_rect(out.rect, w, h),
            "model": MODELS[req["model"]],
        }

    async def run(self, params: Any, progress: ProgressFn | None = None) -> dict[str, Any]:
        if not isinstance(params, dict):
            raise InvalidParams("params must be an object")
        model = params.get("model", "lama")
        if model not in MODELS:
            raise InvalidParams('model must be "lama" or "sdxl"')
        mask = params.get("mask")
        if not isinstance(mask, str) or not mask:
            raise InvalidParams("mask (path of an 8-bit PNG, 255 = remove) is required")
        req = {
            "photo": parse_photo(params.get("photo")),
            "mask": mask,
            "model": model,
            "out_dir": parse_out_dir(params),
        }
        mid = MODELS[model]
        if model == "sdxl" and not sdxl.available():
            raise ModelUnavailable(
                f"model {mid!r} needs the `pro` extra (torch + diffusers): uv sync --extra pro",
                [mid],
            )
        loop = asyncio.get_running_loop()
        notify = progress or (lambda _p: None)
        if not self.manager.is_installed(mid):
            if not params.get("allow_download"):
                raise ModelUnavailable(f"model {mid!r} is not installed", [mid])

            def on_progress(e: dict[str, Any]) -> None:
                loop.call_soon_threadsafe(
                    notify, {"kind": "model.download", "done": e["bytes"], **e}
                )

            try:
                await loop.run_in_executor(
                    self.rt.pool, lambda: self.manager.ensure(mid, on_progress)
                )
            except Exception as e:  # noqa: BLE001
                from ..errors import DownloadFailed, RpcError

                if isinstance(e, RpcError):
                    raise
                raise DownloadFailed(f"{mid}: {e}") from e
        return await loop.run_in_executor(self.rt.pool, self._run, req)
