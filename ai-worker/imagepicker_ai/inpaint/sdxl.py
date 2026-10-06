"""Optional SDXL inpainting backend (`model: "sdxl"`), behind the `pro` extra (torch + diffusers).

Model: `diffusers/stable-diffusion-xl-1.0-inpainting-0.1` (CreativeML OpenRAIL++-M), fp16 variant,
registry id `sdxl-inpaint`, exclusive group `diffusion` (the ModelManager evicts any other
diffusion / VLM model before it loads and never keeps two). VRAM policy: fully resident when the
budget allows (~9 GB), otherwise `enable_model_cpu_offload()` (~3 GB peak, slower); on a CUDA OOM
during a run the pipeline is rebuilt with CPU offload once. Without torch / diffusers the service
answers -32010 naming `sdxl-inpaint`, which Core surfaces as `409 models_missing`.
"""

from __future__ import annotations

import importlib.util
import logging
from pathlib import Path
from typing import Any

import numpy as np

log = logging.getLogger(__name__)

MODEL_ID = "sdxl-inpaint"
SIZE = 1024
PROMPT = "clean natural background, photograph, highly detailed, consistent lighting and texture"
NEGATIVE = "object, person, text, watermark, artifacts, blurry, distorted, low quality"
STEPS = 24
GUIDANCE = 7.5
STRENGTH = 0.99
OFFLOAD_COST_MB = 3500


def available() -> bool:
    """torch + diffusers importable (the `pro` extra is installed)."""
    return all(
        importlib.util.find_spec(m) is not None for m in ("torch", "diffusers", "transformers")
    )


class SdxlHandle:
    def __init__(self, model_dir: Path, offload: bool):
        import torch
        from diffusers import StableDiffusionXLInpaintPipeline

        self.model_id = MODEL_ID
        self.offload = offload
        self.pipe: Any = StableDiffusionXLInpaintPipeline.from_pretrained(
            str(model_dir), torch_dtype=torch.float16, variant="fp16", use_safetensors=True
        )
        if torch.cuda.is_available():
            if offload:
                self.pipe.enable_model_cpu_offload()
            else:
                self.pipe.to("cuda")
            self.providers = ["torch:cuda" + (":offload" if offload else "")]
        else:  # CPU torch: works but takes minutes; fp16 is not supported there
            self.pipe.to(dtype=torch.float32)
            self.providers = ["torch:cpu"]
        self.pipe.set_progress_bar_config(disable=True)
        self.seed = 7

    def fill(self, rgb: np.ndarray, mask: np.ndarray) -> np.ndarray:
        """rgb u8 1024x1024, mask f32 1024x1024 (1 = hole) -> rgb u8 1024x1024."""
        import torch
        from PIL import Image

        img = Image.fromarray(rgb)
        m = Image.fromarray((mask * 255).astype(np.uint8), mode="L")
        gen = torch.Generator(device="cpu").manual_seed(self.seed)
        out = self.pipe(
            prompt=PROMPT,
            negative_prompt=NEGATIVE,
            image=img,
            mask_image=m,
            height=SIZE,
            width=SIZE,
            strength=STRENGTH,
            num_inference_steps=STEPS,
            guidance_scale=GUIDANCE,
            generator=gen,
        ).images[0]
        return np.asarray(out.convert("RGB"), dtype=np.uint8)

    def close(self) -> None:
        self.pipe = None
