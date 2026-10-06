"""SigLIP2 zero-shot heads that reuse the image embedding (no extra image inference).

* scene   8 fixed classes; several English prompts per class, text embeddings averaged
          (then re-normalised); scores = softmax(logit_scale * cos), logit_scale = 112.67
          (SigLIP2-base-patch16-224 learned `logit_scale` = exp(4.7245); the `logit_bias` is a
          constant that cancels in a softmax).
* iqa     CLIP-IQA style antithetical prompt pairs ("sharp" vs "blurry" ...):
          p_pair = sigmoid(scale * (cos(img, good) - cos(img, bad)));  zs = mean over pairs.

Text embeddings of all prompts are computed once and cached under `<models_dir>/_cache/`
(keyed by a hash of the prompt set) so the 550 MB text tower is only needed the first time.
"""

from __future__ import annotations

import hashlib
import json
import logging
import os
from pathlib import Path
from typing import Any

import numpy as np

log = logging.getLogger(__name__)

LOGIT_SCALE = 112.67  # exp(logit_scale) of google/siglip2-base-patch16-224
EMBED_SPACE = "siglip2-base-patch16-224"

SCENE_CLASSES = (
    "portrait",
    "group",
    "landscape",
    "food",
    "architecture",
    "night",
    "pet",
    "other",
)

SCENE_PROMPTS: dict[str, list[str]] = {
    "portrait": [
        "a portrait photo of a person",
        "a close-up photo of a single person's face",
        "a headshot of a man or a woman",
        "a selfie of one person",
        "a photo of one person posing for the camera",
    ],
    "group": [
        "a group photo of several people",
        "a photo of a group of friends together",
        "a family photo with many people",
        "a crowd of people posing together",
        "a photo of three or more people smiling at the camera",
    ],
    "landscape": [
        "a landscape photo of nature",
        "a scenic photo of mountains, a lake or a valley",
        "a photo of the sky, clouds and the horizon",
        "a beautiful seascape or beach at sunset",
        "a wide view of a forest, fields or a coastline",
    ],
    "food": [
        "a photo of food on a plate",
        "a close-up photo of a meal",
        "a photo of a dish served in a restaurant",
        "a photo of a dessert, cake or a drink",
        "a photo of a hamburger, pizza or sushi",
    ],
    "architecture": [
        "a photo of a building",
        "a photo of a famous landmark or a tower",
        "a photo of a city street with buildings",
        "a photo of the interior of a church or a hall",
        "a photo of a bridge, a house or a skyscraper",
    ],
    "night": [
        "a photo taken at night",
        "a dark night scene with city lights",
        "a night sky with stars",
        "a long exposure photo in the dark",
        "a photo of fireworks or neon lights at night",
    ],
    "pet": [
        "a photo of a pet",
        "a photo of a cat",
        "a photo of a dog",
        "a close-up photo of an animal's face",
        "a photo of a pet at home",
    ],
    "other": [
        "a photo of an object on a table",
        "a screenshot or a document",
        "a photo of a car or a vehicle",
        "an abstract pattern or a texture",
        "a photo of a room, furniture or a plant",
    ],
}

# (good, bad) antithetical pairs for the zero-shot technical-quality estimate
IQA_PAIRS: tuple[tuple[str, str], ...] = (
    ("a high quality photo.", "a low quality photo."),
    ("a sharp, in-focus photo.", "a blurry, out-of-focus photo."),
    ("a clean photo with no noise.", "a noisy, grainy photo."),
)


def _l2(x: np.ndarray) -> np.ndarray:
    return x / np.maximum(np.linalg.norm(x, axis=-1, keepdims=True), 1e-12)


def prompt_set_hash() -> str:
    payload = json.dumps(
        {"scene": SCENE_PROMPTS, "iqa": IQA_PAIRS, "space": EMBED_SPACE}, sort_keys=True
    )
    return hashlib.sha256(payload.encode()).hexdigest()[:16]


class PromptBank:
    """Class text vectors for scene + IQA prompts with an on-disk cache."""

    def __init__(self, cache_dir: Path):
        self.cache_file = Path(cache_dir) / f"prompts-{prompt_set_hash()}.npz"
        self._mem: dict[str, np.ndarray] | None = None

    def cached(self) -> bool:
        return self._mem is not None or self.cache_file.is_file()

    def _load_cache(self) -> dict[str, np.ndarray] | None:
        try:
            with np.load(self.cache_file) as z:
                d = {k: z[k] for k in ("scene", "iqa_good", "iqa_bad")}
            if d["scene"].shape[0] == len(SCENE_CLASSES):
                return d
        except Exception as e:  # noqa: BLE001
            log.debug("prompt cache unusable: %s", e)
        return None

    def get(self, embed_text: Any) -> dict[str, np.ndarray]:
        """`embed_text(list[str]) -> (N, D) L2-normalised` is only called on a cache miss."""
        if self._mem is not None:
            return self._mem
        d = self._load_cache() if self.cache_file.is_file() else None
        if d is None:
            texts: list[str] = []
            spans: list[tuple[int, int]] = []
            for c in SCENE_CLASSES:
                spans.append((len(texts), len(texts) + len(SCENE_PROMPTS[c])))
                texts += SCENE_PROMPTS[c]
            n_scene = len(texts)
            texts += [g for g, _ in IQA_PAIRS] + [b for _, b in IQA_PAIRS]
            e = np.asarray(embed_text(texts), dtype=np.float32)
            scene = _l2(np.stack([_l2(e[a:b]).mean(0) for a, b in spans]))
            k = len(IQA_PAIRS)
            d = {
                "scene": scene.astype(np.float32),
                "iqa_good": e[n_scene : n_scene + k],
                "iqa_bad": e[n_scene + k :],
            }
            try:
                self.cache_file.parent.mkdir(parents=True, exist_ok=True)
                tmp = self.cache_file.with_name(self.cache_file.name + ".tmp")
                with open(tmp, "wb") as f:
                    np.savez(f, **d)
                os.replace(tmp, self.cache_file)
            except OSError as ex:
                log.warning("could not cache prompt embeddings: %s", ex)
        self._mem = d
        return d


def scene_scores(emb: np.ndarray, class_vecs: np.ndarray) -> np.ndarray:
    """(N, D) image embeddings -> (N, 8) softmax probabilities in SCENE_CLASSES order."""
    logits = LOGIT_SCALE * (_l2(np.atleast_2d(emb)) @ class_vecs.T)
    logits -= logits.max(axis=1, keepdims=True)
    p = np.exp(logits)
    return p / p.sum(axis=1, keepdims=True)


def iqa_zero_shot(emb: np.ndarray, good: np.ndarray, bad: np.ndarray) -> np.ndarray:
    """(N, D) -> (N,) mean over prompt pairs of sigmoid(scale * (cos_good - cos_bad)), 0-1."""
    e = _l2(np.atleast_2d(emb))
    d = LOGIT_SCALE * (e @ good.T - e @ bad.T)
    return (1.0 / (1.0 + np.exp(-d))).mean(axis=1)


def combine_iqa(nima01: float | None, zs01: float | None) -> float | None:
    """Final 0-1 technical quality: mean of the available signals (NIMA-technical, SigLIP2 zero-shot)."""
    vals = [v for v in (nima01, zs01) if v is not None]
    return float(sum(vals) / len(vals)) if vals else None
