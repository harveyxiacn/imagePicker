"""`besttake.compose` (docs/api-contract-m5.md section B): wires models into `compose_arrays`."""

from __future__ import annotations

import asyncio
import logging
from pathlib import Path
from typing import Any

import cv2
import numpy as np

from ..errors import InvalidParams, ModelUnavailable, OutOfMemory
from ..facemesh import PoseFaceMesh
from ..imgio import (
    Box,
    box_px,
    load_rgb,
    norm_rect,
    parse_box,
    parse_out_dir,
    parse_photo,
    safe_name,
    write_rgba_png,
)
from ..masks.filters import resize_mask
from ..masks.generator import SELFIE, YUNET, MaskGenerator, ProgressFn, bbox_hash
from ..masks.person import select_person
from ..steps.embed import _is_oom
from ..steps.faces import mediapipe_available
from .compose import Components, compose_arrays

log = logging.getLogger(__name__)

FACE_LANDMARKER = "mediapipe-face-landmarker"
MAX_FACE_PX = 1400  # frames are scaled down so the face is at most this wide (rect is unaffected)


class BestTake:
    def __init__(self, masks: MaskGenerator):
        self.masks = masks
        self.manager = masks.manager
        self._mesh: dict[str, PoseFaceMesh] = {}

    def shutdown(self) -> None:
        for m in self._mesh.values():
            m.close()
        self._mesh.clear()

    # ------------------------------------------------------------------ models
    def required_models(self) -> list[str]:
        return [FACE_LANDMARKER, SELFIE, self.masks.subject_model()]

    def optional_models(self) -> list[str]:
        return [YUNET]  # other people's faces (kept out of the matching and the blend region)

    def all_models(self) -> list[str]:
        return self.required_models() + self.optional_models()

    def _mesh_for(self) -> PoseFaceMesh:
        path = str(self.manager.path(FACE_LANDMARKER))
        if path not in self._mesh:
            self._mesh[path] = PoseFaceMesh(path)
        return self._mesh[path]

    def _components(self) -> Components:
        mesh = self._mesh_for()
        selfie = self.masks._selfie()
        net = self.masks._birefnet()

        def parts(crop: np.ndarray) -> np.ndarray:
            return resize_mask(selfie.predict(crop), crop.shape[1], crop.shape[0])

        def matte(sub: np.ndarray, face: Box, others: list[Box]) -> np.ndarray:
            return select_person(sub, face, others, net.predict)

        detect = None
        if self.manager.is_installed(YUNET):
            fa = self.masks._face_analyzer()

            def detect(rgb: np.ndarray) -> list[Box]:
                h, w = rgb.shape[:2]
                s = min(1.0, 1280.0 / max(w, h))
                img = rgb
                if s < 1.0:
                    img = cv2.resize(
                        rgb, (round(w * s), round(h * s)), interpolation=cv2.INTER_AREA
                    )
                return [
                    (f["bbox"][0] * w, f["bbox"][1] * h, f["bbox"][2] * w, f["bbox"][3] * h)
                    for f in fa.detect(img)
                ]

        return Components(mesh.observe, parts, matte, detect)

    # ------------------------------------------------------------------ run
    def _run(self, req: dict[str, Any]) -> dict[str, Any]:
        req["out_dir"].mkdir(parents=True, exist_ok=True)
        base = load_rgb(req["base"])
        src = load_rgb(req["source"])
        bh, bw = base.shape[:2]
        sh, sw = src.shape[:2]
        bf = box_px(req["base_face"], bw, bh)
        sf = box_px(req["source_face"], sw, sh)
        # huge faces: scale both frames down (patch resolution follows; the rect is normalised)
        k = min(1.0, MAX_FACE_PX / max(bf[2], 1.0))
        if k < 1.0:
            base = cv2.resize(base, None, fx=k, fy=k, interpolation=cv2.INTER_AREA)
            src = cv2.resize(src, None, fx=k, fy=k, interpolation=cv2.INTER_AREA)
            bf = tuple(v * k for v in bf)  # type: ignore[assignment]
            sf = tuple(v * k for v in sf)  # type: ignore[assignment]
        ph, pw = base.shape[:2]
        comp = self._components()
        out = compose_arrays(base, src, bf, sf, comp)  # type: ignore[arg-type]
        if out.patch is None or out.rect is None:
            return {"patch": None, "reason": out.reason or "not_composable"}
        name = (
            f"bt_{safe_name(req['base'].photo_id)}_{safe_name(req['source'].photo_id)}"
            f"_{bbox_hash(req['base_face'])}.png"
        )
        path: Path = req["out_dir"] / name
        write_rgba_png(path, out.patch[..., :3], out.patch[..., 3])
        return {
            "patch": str(path),
            "rect": norm_rect(out.rect, pw, ph),
            "quality": out.quality,
        }

    def _run_retry(self, req: dict[str, Any]) -> dict[str, Any]:
        try:
            return self._run(req)
        except Exception as e:  # noqa: BLE001
            if not _is_oom(e):
                raise
            if self.masks.force_cpu:
                raise OutOfMemory(str(e)) from e
            log.warning("OOM in besttake.compose; retrying on CPU")
            self.manager.unload()
            self.masks.force_cpu = True
            return self._run(req)

    async def compose(self, params: Any, progress: ProgressFn | None = None) -> dict[str, Any]:
        if not isinstance(params, dict):
            raise InvalidParams("params must be an object")
        req = {
            "base": parse_photo(params.get("base"), "base"),
            "source": parse_photo(params.get("source"), "source"),
            "base_face": parse_box(params.get("base_face"), "base_face"),
            "source_face": parse_box(params.get("source_face"), "source_face"),
            "out_dir": parse_out_dir(params),
        }
        allow = bool(params.get("allow_download", False))
        loop = asyncio.get_running_loop()
        notify = progress or (lambda _p: None)
        missing = [m for m in self.required_models() if not self.manager.is_installed(m)]
        if missing and allow:
            for mid in missing:

                def on_progress(e: dict[str, Any]) -> None:
                    loop.call_soon_threadsafe(
                        notify, {"kind": "model.download", "done": e["bytes"], **e}
                    )

                try:
                    await loop.run_in_executor(
                        self.masks.pool, lambda m=mid: self.manager.ensure(m, on_progress)
                    )
                except Exception as e:  # noqa: BLE001
                    log.warning("download of %s failed: %s", mid, e)
            missing = [m for m in self.required_models() if not self.manager.is_installed(m)]
        if not mediapipe_available() and FACE_LANDMARKER not in missing:
            missing.append(FACE_LANDMARKER)  # installed weights, but no runtime for them
        if missing:
            raise ModelUnavailable(
                f"besttake.compose needs models: {', '.join(missing)}", list(dict.fromkeys(missing))
            )
        if allow and self.manager.is_installed(YUNET) is False:
            try:
                await loop.run_in_executor(self.masks.pool, lambda: self.manager.ensure(YUNET))
            except Exception:  # noqa: BLE001
                pass
        return await loop.run_in_executor(self.masks.pool, self._run_retry, req)
