"""`hair.rank` and `hair.compose` (docs/api-contract-m5.md section H): wire models into `hair.py`."""

from __future__ import annotations

import asyncio
import logging
from pathlib import Path
from typing import Any

import cv2
import numpy as np

from ..errors import InvalidParams, ModelUnavailable, OutOfMemory
from ..imgio import (
    box_px,
    load_rgb,
    norm_rect,
    parse_box,
    parse_out_dir,
    parse_photo,
    safe_name,
    write_rgba_png,
)
from ..inpaint.service import LAMA
from ..masks.filters import resize_mask
from ..masks.generator import SELFIE, bbox_hash
from ..steps.embed import _is_oom
from ..steps.faces import mediapipe_available
from .compose import Components
from .hair import FillFn, hair_compose_arrays, hair_metrics
from .service import FACE_LANDMARKER, MAX_FACE_PX, BestTake

log = logging.getLogger(__name__)

MAX_RANK_FACES = 24
RANK_SIDE = 2048  # photos are decoded at most this large for ranking (cues are scale-normalised)


MIN_IDENTITY_COS = 0.30  # the core's floor for linked faces (`ip_core::generate::MIN_IDENTITY_COS`)


def check_identity(v: Any) -> None:
    """Defence in depth: compose only a pair the core verified as the same person."""
    if not isinstance(v, dict) or v.get("verified") is not True:
        raise InvalidParams("identity: the core has not verified that both faces are one person")
    cos = v.get("cosine")
    if cos is not None and (not isinstance(cos, (int, float)) or cos < MIN_IDENTITY_COS):
        raise InvalidParams(f"identity: similarity {cos} is below {MIN_IDENTITY_COS}")


class HairTransfer:
    def __init__(self, besttake: BestTake, inpainter: Any | None = None):
        self.bt = besttake
        self.masks = besttake.masks
        self.manager = besttake.manager
        self.inpainter = inpainter

    def required_models(self) -> list[str]:
        return [FACE_LANDMARKER, SELFIE]

    def _components(self) -> Components:
        mesh = self.bt._mesh_for()
        selfie = self.masks._selfie()

        def parts(crop: np.ndarray) -> np.ndarray:
            return resize_mask(selfie.predict(crop), crop.shape[1], crop.shape[0])

        return Components(mesh.observe, parts)

    def _fill(self) -> FillFn | None:
        """LaMa when installed (optional), else None (the compositor falls back to Telea)."""
        if self.inpainter is None or not self.manager.is_installed(LAMA):
            return None
        try:
            from ..inpaint.core import inpaint_arrays

            engine = self.inpainter._lama_engine()[0]
        except Exception:  # noqa: BLE001
            return None

        def fill(rgb: np.ndarray, mask: np.ndarray) -> np.ndarray:
            out = inpaint_arrays(rgb, mask.astype(np.float32), engine, grain=True)
            if out is None:
                return rgb
            res = rgb.copy()
            x, y, w, h = out.rect
            a = out.patch[..., 3:4].astype(np.float32) / 255.0
            res[y : y + h, x : x + w] = np.clip(
                res[y : y + h, x : x + w] * (1 - a) + out.patch[..., :3] * a, 0, 255
            ).astype(np.uint8)
            return res

        return fill

    def _check_models(self) -> None:
        missing = [m for m in self.required_models() if not self.manager.is_installed(m)]
        if not mediapipe_available() and FACE_LANDMARKER not in missing:
            missing.append(FACE_LANDMARKER)
        if missing:
            raise ModelUnavailable(f"hair needs models: {', '.join(missing)}", missing)

    async def _ensure(self, allow: bool) -> None:
        if allow:
            loop = asyncio.get_running_loop()
            for mid in self.required_models():
                if not self.manager.is_installed(mid):
                    try:
                        await loop.run_in_executor(
                            self.masks.pool, lambda m=mid: self.manager.ensure(m)
                        )
                    except Exception as e:  # noqa: BLE001
                        log.warning("download of %s failed: %s", mid, e)
        self._check_models()

    def _retry(self, fn: Any, req: dict[str, Any]) -> dict[str, Any]:
        try:
            return fn(req)
        except Exception as e:  # noqa: BLE001
            if not _is_oom(e) or self.masks.force_cpu:
                if _is_oom(e):
                    raise OutOfMemory(str(e)) from e
                raise
            log.warning("OOM in hair; retrying on CPU")
            self.manager.unload()
            self.masks.force_cpu = True
            return fn(req)

    # ------------------------------------------------------------------ rank
    def _rank(self, req: dict[str, Any]) -> dict[str, Any]:
        comp = self._components()
        results = []
        for item in req["faces"]:
            ref, box = item
            try:
                rgb = load_rgb(ref, RANK_SIDE)
            except Exception as e:  # noqa: BLE001
                log.warning("hair.rank cannot decode photo %s: %s", ref.photo_id, e)
                results.append({"photo_id": ref.photo_id, "ok": False, "reason": "decode_failed"})
                continue
            h, w = rgb.shape[:2]
            try:
                m = hair_metrics(rgb, box_px(box, w, h), comp)
            except Exception as e:  # noqa: BLE001
                if _is_oom(e):
                    raise
                log.warning("hair.rank failed for photo %s: %s", ref.photo_id, e)
                m = {"ok": False, "reason": "error"}
            m["photo_id"] = ref.photo_id
            results.append(m)
        return {"results": results}

    async def rank(self, params: Any) -> dict[str, Any]:
        if not isinstance(params, dict) or not isinstance(params.get("faces"), list):
            raise InvalidParams("params.faces must be a list")
        faces = params["faces"]
        if len(faces) > MAX_RANK_FACES:
            raise InvalidParams(f"at most {MAX_RANK_FACES} faces")
        parsed = []
        for i, f in enumerate(faces):
            if not isinstance(f, dict):
                raise InvalidParams(f"faces[{i}] must be an object")
            parsed.append(
                (
                    parse_photo(f.get("photo"), f"faces[{i}].photo"),
                    parse_box(f.get("face"), f"faces[{i}].face"),
                )
            )
        await self._ensure(bool(params.get("allow_download", False)))
        loop = asyncio.get_running_loop()
        return await loop.run_in_executor(
            self.masks.pool, self._retry, self._rank, {"faces": parsed}
        )

    # ------------------------------------------------------------------ compose
    def _compose(self, req: dict[str, Any]) -> dict[str, Any]:
        req["out_dir"].mkdir(parents=True, exist_ok=True)
        tgt = load_rgb(req["target"])
        don = load_rgb(req["donor"])
        th, tw = tgt.shape[:2]
        dh, dw = don.shape[:2]
        tf = box_px(req["target_face"], tw, th)
        df = box_px(req["donor_face"], dw, dh)
        kt = min(1.0, MAX_FACE_PX / max(tf[2], 1.0))
        if kt < 1.0:
            tgt = cv2.resize(tgt, None, fx=kt, fy=kt, interpolation=cv2.INTER_AREA)
            tf = tuple(v * kt for v in tf)  # type: ignore[assignment]
        kd = min(1.0, MAX_FACE_PX / max(df[2], 1.0))
        if kd < 1.0:
            don = cv2.resize(don, None, fx=kd, fy=kd, interpolation=cv2.INTER_AREA)
            df = tuple(v * kd for v in df)  # type: ignore[assignment]
        ph, pw = tgt.shape[:2]
        out = hair_compose_arrays(tgt, don, tf, df, self._components(), self._fill())  # type: ignore[arg-type]
        if out.patch is None or out.rect is None:
            return {"patch": None, "reason": out.reason or "not_composable"}
        name = (
            f"hair_{safe_name(req['target'].photo_id)}_{safe_name(req['donor'].photo_id)}"
            f"_{bbox_hash(req['target_face'])}.png"
        )
        path: Path = req["out_dir"] / name
        write_rgba_png(path, out.patch[..., :3], out.patch[..., 3])
        return {
            "patch": str(path),
            "rect": norm_rect(out.rect, pw, ph),
            "quality": out.quality,
            "align": out.align,
        }

    async def compose(self, params: Any) -> dict[str, Any]:
        if not isinstance(params, dict):
            raise InvalidParams("params must be an object")
        req = {
            "target": parse_photo(params.get("target"), "target"),
            "donor": parse_photo(params.get("donor"), "donor"),
            "target_face": parse_box(params.get("target_face"), "target_face"),
            "donor_face": parse_box(params.get("donor_face"), "donor_face"),
            "out_dir": parse_out_dir(params),
        }
        check_identity(params.get("identity"))
        await self._ensure(bool(params.get("allow_download", False)))
        loop = asyncio.get_running_loop()
        return await loop.run_in_executor(self.masks.pool, self._retry, self._compose, req)
