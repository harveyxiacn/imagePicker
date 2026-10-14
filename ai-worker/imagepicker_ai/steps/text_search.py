"""`text.embed`: SigLIP2 text-tower vectors for natural-language photo search (PRD F2.9).

Queries go through the same text tower as the zero-shot prompts, so the vectors live in the space
of the stored image embeddings (`analysis.embedding`): cosine = dot product of L2-normalised
vectors, and SigLIP's score is `sigmoid(logit_scale * cos + logit_bias)` (computed by the core).

SigLIP2 was trained on multilingual WebLI data and uses the 256k-token Gemma tokenizer, so a
Chinese query is tokenised natively (no translation step). Quality in Chinese is still lower than
in English; `bench/eval-search.py` measures both.

A small LRU keeps the vectors of recent queries: typing-as-you-search repeats the same text a lot,
and every miss costs a text-tower forward pass (CPU capable, tens of ms; the first one also loads
the ~550 MB model).
"""

from __future__ import annotations

import asyncio
import logging
import threading
from collections import OrderedDict
from collections.abc import Callable
from typing import Any

import numpy as np

from ..errors import InvalidParams, ModelUnavailable
from .embed import EMBED_DIM, TEXT_MODEL, TORCH_MODEL
from .zeroshot import LOGIT_SCALE

log = logging.getLogger(__name__)

MAX_TEXTS = 16
MAX_CHARS = 256
CACHE_SIZE = 256


def normalize_query(text: str) -> str:
    """Whitespace-collapsed query (the tokenizer lower-cases, so the cache key does too)."""
    return " ".join(text.split()).casefold()


def parse(params: Any) -> tuple[list[str], bool]:
    if not isinstance(params, dict):
        raise InvalidParams("params must be an object")
    texts = params.get("texts")
    if not isinstance(texts, list) or not texts or not all(isinstance(t, str) for t in texts):
        raise InvalidParams("texts must be a non-empty array of strings")
    if len(texts) > MAX_TEXTS:
        raise InvalidParams(f"texts takes at most {MAX_TEXTS} queries")
    cleaned = [" ".join(t.split()) for t in texts]
    if any(not t for t in cleaned):
        raise InvalidParams("texts must not contain empty queries")
    if any(len(t) > MAX_CHARS for t in cleaned):
        raise InvalidParams(f"a query is limited to {MAX_CHARS} characters")
    return cleaned, bool(params.get("allow_download", False))


class TextSearch:
    """LRU-cached query embedding on top of `embed_fn(list[str]) -> (N, D) L2-normalised`."""

    def __init__(
        self,
        embed_fn: Callable[[list[str]], np.ndarray],
        *,
        cache_size: int = CACHE_SIZE,
    ):
        self._embed = embed_fn
        self._size = max(1, cache_size)
        self._cache: OrderedDict[str, np.ndarray] = OrderedDict()
        self._lock = threading.Lock()
        self.model_calls = 0  # forward passes (cache misses), for tests and diagnostics

    def cached(self, text: str) -> bool:
        with self._lock:
            return normalize_query(text) in self._cache

    def embed(self, texts: list[str]) -> tuple[np.ndarray, list[bool]]:
        """Vectors in request order and, per query, whether it came from the cache."""
        keys = [normalize_query(t) for t in texts]
        with self._lock:
            hit = [k in self._cache for k in keys]
            for k, h in zip(keys, hit, strict=True):
                if h:
                    self._cache.move_to_end(k)
        misses: list[str] = []
        seen: set[str] = set()
        for t, k, h in zip(texts, keys, hit, strict=True):
            if not h and k not in seen:
                seen.add(k)
                misses.append(t)
        if misses:
            vecs = np.asarray(self._embed(misses), dtype=np.float32)
            self.model_calls += 1
            with self._lock:
                for t, v in zip(misses, vecs, strict=True):
                    self._cache[normalize_query(t)] = v
                    self._cache.move_to_end(normalize_query(t))
                while len(self._cache) > self._size:
                    self._cache.popitem(last=False)
        with self._lock:
            out = np.stack([self._cache[k] for k in keys]).astype(np.float32)
        return out, hit


def missing_models(manager: Any) -> list[str]:
    if manager.is_installed(TEXT_MODEL) or manager.is_installed(TORCH_MODEL):
        return []
    return [TEXT_MODEL]


async def text_embed(search: TextSearch, manager: Any, params: Any, pool: Any) -> dict[str, Any]:
    texts, allow_download = parse(params)
    loop = asyncio.get_running_loop()
    if not all(search.cached(t) for t in texts):
        missing = missing_models(manager)
        if missing and allow_download:
            for m in missing:
                await loop.run_in_executor(pool, manager.ensure, m)
            missing = missing_models(manager)
        if missing:
            raise ModelUnavailable(f"models not installed: {', '.join(missing)}", missing)
    vecs, cached = await loop.run_in_executor(pool, search.embed, texts)
    return {
        "model": TEXT_MODEL,
        "dim": int(vecs.shape[1]) if vecs.ndim == 2 else EMBED_DIM,
        "logit_scale": LOGIT_SCALE,
        "vectors": np.round(vecs, 6).astype(float).tolist(),
        "cached": cached,
    }
