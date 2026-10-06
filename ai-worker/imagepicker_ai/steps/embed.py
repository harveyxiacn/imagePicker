"""SigLIP2 image / text embeddings (ONNX Runtime first, transformers+torch fallback).

Image tower: squash-resize to 224x224 (bilinear, as in the HF SiglipImageProcessor), scale to
[-1, 1], `pooler_output` (768-d) -> L2 normalize.  Text tower: lower-cased text, tokenizer.json
(pads to 64 tokens), `pooler_output` -> L2 normalize.  Image and text vectors live in the same
space; zero-shot / search score = cosine (SigLIP logits = scale * cos + bias, monotone in cos).
"""

from __future__ import annotations

import logging
import threading
from pathlib import Path
from typing import Any

import numpy as np
from PIL import Image

from ..errors import ModelUnavailable, OutOfMemory
from ..hw import HardwareInfo
from ..models.manager import ModelManager
from ..models.registry import ModelSpec

log = logging.getLogger(__name__)

IMG_SIZE = 224
TEXT_LEN = 64
EMBED_DIM = 768

ONNX_IMAGE_MODELS_GPU = ["siglip2-base-fp16", "siglip2-base"]
ONNX_IMAGE_MODELS_CPU = ["siglip2-base", "siglip2-base-fp16"]
TEXT_MODEL = "siglip2-base-text"
TORCH_MODEL = "siglip2-base-torch"


def prepare_image(rgb: np.ndarray) -> np.ndarray:
    """Any-size RGB uint8 -> 224x224x3 uint8 (thread-safe, run in decode threads)."""
    im = Image.fromarray(rgb)
    return np.asarray(im.resize((IMG_SIZE, IMG_SIZE), Image.Resampling.BILINEAR), dtype=np.uint8)


def _l2(x: np.ndarray) -> np.ndarray:
    n = np.linalg.norm(x, axis=1, keepdims=True)
    return x / np.maximum(n, 1e-12)


def _as_array(out: Any) -> np.ndarray:
    # transformers >= 5 returns a ModelOutput with .pooler_output, older versions a tensor
    t = getattr(out, "pooler_output", out)
    return t.float().cpu().numpy()


def _is_oom(e: BaseException) -> bool:
    s = str(e).lower()
    return any(
        k in s
        for k in (
            "out of memory",
            "failed to allocate",
            "bfcarena",
            "cudaerrormemoryallocation",
            "oom",
        )
    )


def _make_ort_session(path: Path, providers: list[str]) -> Any:
    import onnxruntime as ort

    if "CUDAExecutionProvider" in providers and hasattr(ort, "preload_dlls"):
        try:
            ort.preload_dlls()  # find CUDA/cuDNN from the nvidia-* wheels
        except Exception:  # noqa: BLE001
            pass
    so = ort.SessionOptions()
    so.graph_optimization_level = ort.GraphOptimizationLevel.ORT_ENABLE_ALL
    so.log_severity_level = 3
    prov: list[Any] = []
    for p in providers:
        if p == "CUDAExecutionProvider":
            prov.append((p, {"arena_extend_strategy": "kSameAsRequested"}))
        else:
            prov.append(p)
    return ort.InferenceSession(str(path), sess_options=so, providers=prov)


class _OrtVision:
    def __init__(self, session: Any, model_id: str):
        self.session = session
        self.model_id = model_id
        self.input_name = session.get_inputs()[0].name
        outs = [o.name for o in session.get_outputs()]
        self.output_name = "pooler_output" if "pooler_output" in outs else outs[-1]

    @property
    def providers(self) -> list[str]:
        return self.session.get_providers()

    def run(self, x: np.ndarray) -> np.ndarray:
        return self.session.run([self.output_name], {self.input_name: x})[0]

    def close(self) -> None:
        self.session = None


class _TorchModel:
    """transformers fallback: one object serving both towers."""

    def __init__(self, directory: Path, device: str):
        import torch
        from transformers import AutoModel, AutoTokenizer

        self.torch = torch
        self.device = device
        self.model = AutoModel.from_pretrained(
            str(directory), dtype=torch.float16 if device != "cpu" else torch.float32
        )
        self.model.to(device).eval()
        self.tokenizer = AutoTokenizer.from_pretrained(str(directory))
        self.model_id = TORCH_MODEL
        self.providers = [f"torch:{device}"]

    def run_image(self, x: np.ndarray) -> np.ndarray:
        t = self.torch.from_numpy(x).to(self.device, dtype=self.model.dtype)
        with self.torch.inference_mode():
            return _as_array(self.model.get_image_features(pixel_values=t))

    def run_text(self, texts: list[str]) -> np.ndarray:
        tok = self.tokenizer(
            texts, padding="max_length", max_length=TEXT_LEN, truncation=True, return_tensors="pt"
        )
        with self.torch.inference_mode():
            return _as_array(
                self.model.get_text_features(input_ids=tok["input_ids"].to(self.device))
            )

    def close(self) -> None:
        self.model = None
        if self.device == "cuda":
            self.torch.cuda.empty_cache()


class _OrtText:
    def __init__(self, session: Any, tokenizer: Any, model_id: str):
        self.session = session
        self.tokenizer = tokenizer
        self.model_id = model_id
        outs = [o.name for o in session.get_outputs()]
        self.output_name = "pooler_output" if "pooler_output" in outs else outs[-1]

    def run(self, texts: list[str]) -> np.ndarray:
        enc = self.tokenizer.encode_batch([t.lower() for t in texts])
        ids = np.array(
            [e.ids[:TEXT_LEN] + [0] * max(0, TEXT_LEN - len(e.ids)) for e in enc], dtype=np.int64
        )
        return self.session.run([self.output_name], {"input_ids": ids})[0]

    def close(self) -> None:
        self.session = None


class Embedder:
    """Owns model selection, loading through the ModelManager, batching and OOM backoff."""

    def __init__(
        self, manager: ModelManager, hw: HardwareInfo, batch_size: int = 32, backend: str = "auto"
    ):
        self.manager = manager
        self.hw = hw
        self.batch_size = batch_size
        self.backend = backend  # auto | onnx | torch
        self.force_cpu = hw.device == "cpu"
        self._lock = threading.Lock()  # one inference at a time per embedder
        self.active_model: str | None = None
        self.active_providers: list[str] = []

    # ---- model selection
    def required_models(self) -> list[str]:
        """Model ids that must be installed for the image tower (first = preferred)."""
        mid = self._pick_image_model()
        return [mid]

    def _providers(self) -> list[str]:
        return ["CPUExecutionProvider"] if self.force_cpu else self.hw.providers

    def _pick_image_model(self) -> str:
        if self.backend == "torch":
            return TORCH_MODEL
        order = (
            ONNX_IMAGE_MODELS_CPU
            if self.force_cpu or self.hw.device in ("cpu", "mps")
            else ONNX_IMAGE_MODELS_GPU
        )
        for mid in order:
            if self.manager.is_installed(mid):
                return mid
        if self.backend == "auto" and self.manager.is_installed(TORCH_MODEL):
            return TORCH_MODEL
        return order[0]

    def _load_image_model(self) -> Any:
        mid = self._pick_image_model()
        if mid == TORCH_MODEL:

            def load_torch(spec: ModelSpec, path: Path) -> Any:
                dev = "cuda" if (not self.force_cpu and self.hw.device == "cuda") else "cpu"
                return _TorchModel(self.manager.dir(spec.id), dev)

            h = self.manager.acquire(mid, load_torch)
        else:

            def load_onnx(spec: ModelSpec, path: Path) -> Any:
                return _OrtVision(_make_ort_session(path, self._providers()), spec.id)

            try:
                h = self.manager.acquire(mid, load_onnx)
            except ModelUnavailable:
                raise
            except OutOfMemory:
                raise
            except Exception as e:  # noqa: BLE001  onnxruntime missing / broken
                if self.backend == "auto" and self.manager.is_installed(TORCH_MODEL):
                    log.warning("ONNX embed failed (%s); using torch fallback", e)
                    self.backend = "torch"
                    return self._load_image_model()
                raise
        self.active_model = mid
        self.active_providers = list(getattr(h, "providers", []))
        return h

    # ---- image embeddings
    def embed_prepared(self, prepared: list[np.ndarray]) -> np.ndarray:
        """list of 224x224x3 uint8 -> [N, 768] float32, L2-normalized. Batched with OOM backoff."""
        if not prepared:
            return np.zeros((0, EMBED_DIM), np.float32)
        with self._lock:
            out: list[np.ndarray] = []
            i = 0
            while i < len(prepared):
                bs = max(1, self.batch_size)
                chunk = prepared[i : i + bs]
                x = (np.stack(chunk).astype(np.float32) * (1 / 127.5) - 1.0).transpose(0, 3, 1, 2)
                try:
                    h = self._load_image_model()
                    y = (
                        h.run_image(x)
                        if isinstance(h, _TorchModel)
                        else h.run(np.ascontiguousarray(x))
                    )
                except Exception as e:  # noqa: BLE001
                    if not _is_oom(e):
                        raise
                    if self.batch_size > 1:
                        self.batch_size = max(1, self.batch_size // 2)
                        log.warning("OOM during embedding; batch size -> %d", self.batch_size)
                        continue
                    if not self.force_cpu:
                        log.warning("OOM at batch size 1; falling back to CPU")
                        self.manager.unload()
                        self.force_cpu = True
                        self.batch_size = 8
                        continue
                    raise OutOfMemory(str(e)) from e
                out.append(np.asarray(y, dtype=np.float32))
                i += len(chunk)
            return _l2(np.concatenate(out, axis=0))

    def embed_images(self, rgbs: list[np.ndarray]) -> np.ndarray:
        return self.embed_prepared([prepare_image(a) for a in rgbs])

    # ---- text embeddings
    def embed_text(self, texts: list[str]) -> np.ndarray:
        """list[str] -> [N, 768] float32, L2-normalized (same space as the image vectors)."""
        if not texts:
            return np.zeros((0, EMBED_DIM), np.float32)
        with self._lock:
            if self.backend == "torch" or (
                self.backend == "auto"
                and not self.manager.is_installed(TEXT_MODEL)
                and self.manager.is_installed(TORCH_MODEL)
            ):

                def load_torch(spec: ModelSpec, path: Path) -> Any:
                    dev = "cuda" if (not self.force_cpu and self.hw.device == "cuda") else "cpu"
                    return _TorchModel(self.manager.dir(spec.id), dev)

                h = self.manager.acquire(TORCH_MODEL, load_torch)
                return _l2(h.run_text(texts))

            def load_onnx(spec: ModelSpec, path: Path) -> Any:
                from tokenizers import Tokenizer

                tok = Tokenizer.from_file(str(self.manager.dir(spec.id) / "tokenizer.json"))
                tok.enable_padding(length=TEXT_LEN, pad_id=0, pad_token="<pad>")
                tok.enable_truncation(TEXT_LEN)
                return _OrtText(_make_ort_session(path, self._providers()), tok, spec.id)

            h = self.manager.acquire(TEXT_MODEL, load_onnx)
            return _l2(np.concatenate([h.run(texts[i : i + 64]) for i in range(0, len(texts), 64)]))
