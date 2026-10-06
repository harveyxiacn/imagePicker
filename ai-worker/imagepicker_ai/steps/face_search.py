"""`faces.embed`: detect faces in one image and return their AuraFace embeddings (face search).

Same decode, YuNet detection, 5-point alignment and AuraFace model as the `identity` step, so a
query embedding is directly comparable (cosine = dot product) with the stored ones. No files.
"""

from __future__ import annotations

import asyncio
import logging
from typing import Any

import numpy as np

from ..decode import DecodeError, load_image
from ..errors import DECODE_FAILED, InvalidParams, ModelUnavailable, RpcError
from ..pipeline import Analyzer
from .identity import MODEL_ID as AURAFACE
from .identity import kps_from_bbox

log = logging.getLogger(__name__)

YUNET = "yunet"
DEFAULT_SIZE = 1024
MAX_FACES = 30


def parse(params: Any) -> tuple[str, int | None, int, bool]:
    if not isinstance(params, dict):
        raise InvalidParams("params must be an object")
    path = params.get("path")
    if not isinstance(path, str) or not path:
        raise InvalidParams("path is required")
    orient = params.get("orientation")
    if orient is not None and (not isinstance(orient, int) or not 1 <= orient <= 8):
        raise InvalidParams("orientation must be an EXIF value 1-8")
    size = params.get("analysis_size", DEFAULT_SIZE)
    if not isinstance(size, int) or isinstance(size, bool) or not 128 <= size <= 8192:
        raise InvalidParams("analysis_size must be an integer in [128, 8192]")
    return path, orient, size, bool(params.get("allow_download", False))


def embed_image(
    analyzer: Analyzer, path: str, orientation: int | None, size: int
) -> dict[str, Any]:
    try:
        dec = load_image(path, size, orientation)
    except DecodeError as e:
        raise RpcError(DECODE_FAILED, str(e), "decode_failed") from e
    faces = analyzer._face_analyzer().detect(dec.rgb)[:MAX_FACES]
    if not faces:
        return {"faces": [], "embeddings": []}
    kps = [
        f["_kps"] if f.get("_kps") is not None else kps_from_bbox(f["bbox"], dec.width, dec.height)
        for f in faces
    ]
    emb = analyzer.identity.embed_faces(dec.rgb, kps)
    return {
        "faces": [{"bbox": f["bbox"], "det_score": f["det_score"]} for f in faces],
        "embeddings": np.round(emb, 6).astype(float).tolist(),
    }


async def faces_embed(
    analyzer: Analyzer, params: Any, pool: Any, progress: Any = None
) -> dict[str, Any]:
    path, orient, size, allow_download = parse(params)
    loop = asyncio.get_running_loop()
    mgr = analyzer.manager
    missing = [m for m in (YUNET, AURAFACE) if not mgr.is_installed(m)]
    if missing and allow_download:
        for m in missing:
            await loop.run_in_executor(pool, mgr.ensure, m)
        missing = [m for m in missing if not mgr.is_installed(m)]
    if missing:
        raise ModelUnavailable(f"models not installed: {', '.join(missing)}", missing)
    return await loop.run_in_executor(pool, embed_image, analyzer, path, orient, size)
