"""`enhance.run` (docs/api-contract-m5.md section B): denoise / face_restore / upscale."""

from __future__ import annotations

import asyncio
import logging
from pathlib import Path
from typing import Any

import cv2
import numpy as np

from ..errors import DownloadFailed, InvalidParams, ModelUnavailable, RpcError
from ..facemesh import PoseFaceMesh
from ..imgio import (
    box_px,
    load_rgb,
    norm_rect,
    parse_box,
    parse_out_dir,
    parse_photo,
    safe_name,
    write_png,
    write_rgba_png,
)
from ..masks.generator import YUNET, MaskGenerator, ProgressFn
from ..models.manager import ModelManager
from ..runtime import MIN_TILE, GenRuntime, OrtHandle
from ..steps.faces import FaceAnalyzer, mediapipe_available
from .ops import auto_strength, denoise_tiled, restore_face, upscale_tiled

log = logging.getLogger(__name__)

DENOISE = "scunet-color-real-psnr"
FACE_RESTORE = "gfpgan-v1.4"
FACE_LANDMARKER = "mediapipe-face-landmarker"
UPSCALE = {2: ("realesrgan-x2-fp16", "realesrgan-x2"), 4: ("realesrgan-x4-fp16", "realesrgan-x4")}
OPS = ("denoise", "face_restore", "upscale")
MAX_FACES = 24
MAX_OUTPUT_PIXELS = 400_000_000
DENOISE_TILE = 512
UPSCALE_TILE = 512
CPU_TILE = 256


class Enhancer:
    def __init__(self, runtime: GenRuntime, masks: MaskGenerator):
        self.rt = runtime
        self.masks = masks
        self.manager: ModelManager = runtime.manager
        self._mesh: dict[str, PoseFaceMesh] = {}

    def shutdown(self) -> None:
        for m in self._mesh.values():
            m.close()
        self._mesh.clear()

    # ------------------------------------------------------------------ models
    def upscale_model(self, scale: int) -> str:
        """fp16 on GPU, fp32 on CPU; whichever is installed wins over the preferred one."""
        fp16, fp32 = UPSCALE[scale]
        order = [fp32, fp16] if self.rt.cpu_only else [fp16, fp32]
        for mid in order:
            if self.manager.is_installed(mid):
                return mid
        return order[0]

    def required_models(self, op: str, scale: int = 2) -> list[str]:
        if op == "denoise":
            return [DENOISE]
        if op == "face_restore":
            return [FACE_RESTORE, FACE_LANDMARKER]
        if op == "upscale":
            return [self.upscale_model(scale)]
        raise KeyError(op)

    def _mesh_for(self) -> PoseFaceMesh:
        path = str(self.manager.path(FACE_LANDMARKER))
        if path not in self._mesh:
            self._mesh[path] = PoseFaceMesh(path)
        return self._mesh[path]

    # ------------------------------------------------------------------ ops
    def _denoise(self, req: dict[str, Any], rgb: np.ndarray, notify: ProgressFn) -> dict[str, Any]:
        s = req["strength"] if req["strength"] is not None else auto_strength(rgb)

        def work(h: OrtHandle, tile: int) -> np.ndarray:
            def net(x: np.ndarray) -> np.ndarray:
                return h.run({"image": x})

            def prog(i: int, n: int) -> None:
                notify({"kind": "enhance", "done": i, "total": n})

            return denoise_tiled(rgb, net, tile, s, prog)

        out = self.rt.run_with_backoff(DENOISE, work, DENOISE_TILE, CPU_TILE)
        path = req["out_dir"] / f"denoise_{safe_name(req['photo'].photo_id)}.png"
        write_rgba_png(path, out, np.full(out.shape[:2], 255, np.uint8))
        return {"patch": str(path), "rect": [0, 0, 1, 1]}

    def _upscale(self, req: dict[str, Any], rgb: np.ndarray, notify: ProgressFn) -> dict[str, Any]:
        scale = req["scale"]
        mid = self.upscale_model(scale)
        s = 1.0 if req["strength"] is None else req["strength"]

        def work(h: OrtHandle, tile: int) -> np.ndarray:
            def net(x: np.ndarray) -> np.ndarray:
                return h.run({h.input_name: x})

            def prog(i: int, n: int) -> None:
                notify({"kind": "enhance", "done": i, "total": n})

            return upscale_tiled(rgb, net, scale, tile, s, prog)

        out = self.rt.run_with_backoff(mid, work, UPSCALE_TILE, CPU_TILE)
        path = req["out_dir"] / f"upscale_{safe_name(req['photo'].photo_id)}_x{scale}.png"
        write_png(path, out)
        return {"image": str(path)}

    def _detect_faces(self, rgb: np.ndarray) -> list[list[float]]:
        fa: FaceAnalyzer | None = self.masks._face_analyzer()
        if fa is None:
            return []
        h, w = rgb.shape[:2]
        s = min(1.0, 1280.0 / max(w, h))
        img = (
            rgb
            if s >= 1
            else cv2.resize(rgb, (round(w * s), round(h * s)), interpolation=cv2.INTER_AREA)
        )
        return [f["bbox"] for f in fa.detect(img)]

    def _face_restore(
        self, req: dict[str, Any], rgb: np.ndarray, notify: ProgressFn
    ) -> dict[str, Any]:
        h, w = rgb.shape[:2]
        faces = req["faces"]
        if faces is None:
            faces = self._detect_faces(rgb)
        faces = faces[:MAX_FACES]
        s = 0.8 if req["strength"] is None else req["strength"]
        mesh = self._mesh_for()
        patches: list[dict[str, Any]] = []
        skipped: list[dict[str, Any]] = []
        for i, fb in enumerate(faces):
            obs = mesh.observe(rgb, box_px(fb, w, h))
            if obs is None:
                skipped.append({"face_index": i, "reason": "face_not_found"})
                continue

            def work(hd: OrtHandle, _tile: int, _obs=obs) -> Any:
                def net(inp: np.ndarray, _hd: OrtHandle = hd) -> np.ndarray:
                    return _hd.run({_hd.input_name: inp})

                return restore_face(rgb, _obs, net, s)

            res = self.rt.run_with_backoff(FACE_RESTORE, work, MIN_TILE)
            if res is None:
                skipped.append({"face_index": i, "reason": "face_too_small"})
                continue
            path = req["out_dir"] / f"face_restore_{safe_name(req['photo'].photo_id)}_{i}.png"
            write_rgba_png(path, res.rgba[..., :3], res.rgba[..., 3])
            patches.append(
                {
                    "patch": str(path),
                    "rect": norm_rect(res.rect, w, h),
                    "face_index": i,
                }
            )
            notify({"kind": "enhance", "done": i + 1, "total": len(faces)})
        out: dict[str, Any] = {"patches": patches}
        if skipped:
            out["skipped"] = skipped
        return out

    def _run(self, req: dict[str, Any], notify: ProgressFn) -> dict[str, Any]:
        Path(req["out_dir"]).mkdir(parents=True, exist_ok=True)
        rgb = load_rgb(req["photo"])
        op = req["op"]
        if op == "upscale":
            h, w = rgb.shape[:2]
            if h * w * req["scale"] ** 2 > MAX_OUTPUT_PIXELS:
                raise InvalidParams(
                    f"upscale x{req['scale']} of {w}x{h} exceeds {MAX_OUTPUT_PIXELS // 10**6} MP"
                )
            return self._upscale(req, rgb, notify)
        if op == "denoise":
            return self._denoise(req, rgb, notify)
        return self._face_restore(req, rgb, notify)

    # ------------------------------------------------------------------ entry
    async def run(self, params: Any, progress: ProgressFn | None = None) -> dict[str, Any]:
        if not isinstance(params, dict):
            raise InvalidParams("params must be an object")
        op = params.get("op")
        if op not in OPS:
            raise InvalidParams(f"op must be one of {', '.join(OPS)}")
        strength = params.get("strength")
        if strength is not None and (
            not isinstance(strength, (int, float))
            or isinstance(strength, bool)
            or not 0 <= strength <= 1
        ):
            raise InvalidParams("strength must be a number in [0, 1]")
        scale = params.get("scale", 2)
        if op == "upscale" and scale not in (2, 4):
            raise InvalidParams("scale must be 2 or 4")
        faces = params.get("faces")
        if faces is not None:
            if not isinstance(faces, list):
                raise InvalidParams("faces must be an array of [x, y, w, h]")
            faces = [parse_box(f, f"faces[{i}]") for i, f in enumerate(faces)]
        req = {
            "photo": parse_photo(params.get("photo")),
            "op": op,
            "strength": None if strength is None else float(strength),
            "scale": scale,
            "faces": faces,
            "out_dir": parse_out_dir(params),
        }
        loop = asyncio.get_running_loop()
        notify = progress or (lambda _p: None)
        need = self.required_models(op, scale)
        missing = [m for m in need if not self.manager.is_installed(m)]
        if missing and params.get("allow_download"):
            for mid in missing:

                def on_progress(e: dict[str, Any]) -> None:
                    loop.call_soon_threadsafe(
                        notify, {"kind": "model.download", "done": e["bytes"], **e}
                    )

                try:
                    await loop.run_in_executor(
                        self.rt.pool, lambda m=mid: self.manager.ensure(m, on_progress)
                    )
                except RpcError:
                    raise
                except Exception as e:  # noqa: BLE001
                    raise DownloadFailed(f"{mid}: {e}") from e
            missing = [m for m in need if not self.manager.is_installed(m)]
        if op == "face_restore" and not mediapipe_available() and FACE_LANDMARKER not in missing:
            missing.append(FACE_LANDMARKER)
        if missing:
            raise ModelUnavailable(f"enhance.{op} needs models: {', '.join(missing)}", missing)
        if op == "face_restore" and req["faces"] is None and params.get("allow_download"):
            try:
                await loop.run_in_executor(self.rt.pool, lambda: self.manager.ensure(YUNET))
            except Exception:  # noqa: BLE001
                pass
        return await loop.run_in_executor(self.rt.pool, self._run, req, notify)
