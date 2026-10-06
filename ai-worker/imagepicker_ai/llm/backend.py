"""onnxruntime-genai backend (greedy decoding, optional grammar-constrained JSON).

Runtime choice (see README "Local AI assistant"): onnxruntime-genai ships wheels for Windows, Linux
and macOS (CPU) and Windows/Linux (CUDA) and needs no compiler. `onnxruntime-genai-cuda` depends on
`onnxruntime-gpu`, the package the `cuda` extra already installs, so both coexist in one venv.

A backend object only has to provide the two `generate` signatures below, which is how the unit
tests inject stubs.
"""

from __future__ import annotations

import importlib.util
import json
import logging
import threading
import time
from pathlib import Path
from typing import Any, Protocol

import cv2
import numpy as np

log = logging.getLogger(__name__)


class GenerationTimeout(Exception):
    pass


class GenerationCancelled(Exception):
    pass


class TextBackend(Protocol):
    def generate(
        self,
        messages: list[dict[str, str]],
        schema: dict[str, Any] | None,
        max_tokens: int,
        deadline: float,
        cancel: threading.Event | None = None,
    ) -> str: ...


class VisionBackend(Protocol):
    def generate(
        self,
        rgb: np.ndarray,
        prompt: str,
        schema: dict[str, Any] | None,
        max_tokens: int,
        deadline: float,
        cancel: threading.Event | None = None,
    ) -> str: ...


def genai_available() -> bool:
    """The `llm` / `llm-cuda` extra is installed."""
    return importlib.util.find_spec("onnxruntime_genai") is not None


def cuda_available() -> bool:
    if not genai_available():
        return False
    try:
        import onnxruntime as ort

        if "CUDAExecutionProvider" not in ort.get_available_providers():
            return False
        import onnxruntime_genai as og

        return bool(og.is_cuda_available())
    except Exception:  # noqa: BLE001
        return False


# Phi-3.5-vision chat format (the processor expands <|image_1|> into the image tokens)
VISION_TEMPLATE = "<|user|>\n<|image_1|>\n{prompt}<|end|>\n<|assistant|>\n"
MAX_LENGTH_VISION = 8192


class GenaiHandle:
    """One loaded genai model (text-only or vision) + its tokenizer / processor."""

    def __init__(self, model_dir: Path, provider: str, vision: bool, model_id: str):
        import onnxruntime as ort

        if provider == "cuda" and hasattr(ort, "preload_dlls"):
            try:
                ort.preload_dlls()  # CUDA / cuDNN from the nvidia-* wheels
            except Exception:  # noqa: BLE001
                pass
        import onnxruntime_genai as og

        self.og = og
        self.model_id = model_id
        self.provider = provider
        self.vision = vision
        cfg = og.Config(str(model_dir))
        cfg.clear_providers()
        if provider != "cpu":
            cfg.append_provider(provider)
        self.model: Any = og.Model(cfg)
        self.tok: Any = og.Tokenizer(self.model)
        self.proc: Any = self.model.create_multimodal_processor() if vision else None
        self.providers = [f"genai:{provider}"]
        self._lock = threading.Lock()
        self.last_stats: dict[str, Any] = {}

    def close(self) -> None:
        self.proc = None
        self.tok = None
        self.model = None

    # ------------------------------------------------------------------ internals
    def _params(self, max_length: int, schema: dict[str, Any] | None) -> tuple[Any, bool]:
        p = self.og.GeneratorParams(self.model)
        p.set_search_options(max_length=max_length, do_sample=False, temperature=0.0, top_k=1)
        guided = False
        if schema is not None and hasattr(p, "set_guidance"):
            try:
                p.set_guidance("json_schema", json.dumps(schema, ensure_ascii=False))
                guided = True
            except Exception as e:  # noqa: BLE001
                log.warning("grammar constraint rejected (%s); decoding unconstrained", e)
        return p, guided

    def _decode_loop(
        self, g: Any, max_tokens: int, deadline: float, cancel: threading.Event | None
    ) -> list[int]:
        out: list[int] = []
        while not g.is_done() and len(out) < max_tokens:
            if cancel is not None and cancel.is_set():
                raise GenerationCancelled()
            if time.monotonic() > deadline:
                raise GenerationTimeout(f"timed out after {len(out)} tokens")
            g.generate_next_token()
            out.append(int(g.get_next_tokens()[0]))
        return out

    # ------------------------------------------------------------------ text
    def generate(
        self,
        messages: list[dict[str, str]],
        schema: dict[str, Any] | None,
        max_tokens: int,
        deadline: float,
        cancel: threading.Event | None = None,
    ) -> str:
        assert not self.vision, "text generate() on a vision handle: use generate_image()"
        t0 = time.monotonic()
        with self._lock:
            prompt = self.tok.apply_chat_template(
                json.dumps(messages, ensure_ascii=False), add_generation_prompt=True
            )
            ids = self.tok.encode(prompt)
            params, guided = self._params(len(ids) + max_tokens + 1, schema)
            g = self.og.Generator(self.model, params)
            g.append_tokens(ids)
            out = self._decode_loop(g, max_tokens, deadline, cancel)
            text = self.tok.decode(out) if out else ""
        self.last_stats = {
            "prompt_tokens": len(ids),
            "new_tokens": len(out),
            "guided": guided,
            "seconds": round(time.monotonic() - t0, 3),
        }
        return text

    # ------------------------------------------------------------------ vision
    def generate_image(
        self,
        rgb: np.ndarray,
        prompt: str,
        schema: dict[str, Any] | None,
        max_tokens: int,
        deadline: float,
        cancel: threading.Event | None = None,
    ) -> str:
        assert self.vision
        t0 = time.monotonic()
        ok, buf = cv2.imencode(
            ".jpg", cv2.cvtColor(rgb, cv2.COLOR_RGB2BGR), [cv2.IMWRITE_JPEG_QUALITY, 92]
        )
        if not ok:
            raise RuntimeError("image encode failed")
        with self._lock:
            images = self.og.Images.open_bytes(buf.tobytes())
            inputs = self.proc(VISION_TEMPLATE.format(prompt=prompt), images=images)
            params, guided = self._params(MAX_LENGTH_VISION, schema)
            g = self.og.Generator(self.model, params)
            g.set_inputs(inputs)
            prompt_tokens = int(g.token_count())
            out = self._decode_loop(g, max_tokens, deadline, cancel)
            text = self.proc.decode(out) if out else ""
        self.last_stats = {
            "prompt_tokens": prompt_tokens,
            "new_tokens": len(out),
            "guided": guided,
            "seconds": round(time.monotonic() - t0, 3),
        }
        return text


class _VisionAdapter:
    """Gives a vision `GenaiHandle` the `VisionBackend.generate` signature."""

    def __init__(self, handle: GenaiHandle):
        self.handle = handle

    def generate(self, rgb, prompt, schema, max_tokens, deadline, cancel=None) -> str:  # noqa: ANN001
        return self.handle.generate_image(rgb, prompt, schema, max_tokens, deadline, cancel)


def vision_backend(handle: GenaiHandle) -> VisionBackend:
    return _VisionAdapter(handle)
