from __future__ import annotations

import io
import os
import urllib.request
from pathlib import Path

import cv2
import numpy as np
import pytest
from PIL import Image

AI_WORKER_ROOT = Path(__file__).resolve().parents[1]


def synth_image(seed: int, size: tuple[int, int] = (640, 480)) -> np.ndarray:
    """Photo-ish synthetic RGB uint8: smooth coloured blobs + shapes + fine texture."""
    rng = np.random.default_rng(seed)
    w, h = size
    low = rng.random((6, 8, 3)).astype(np.float32)
    img = cv2.resize(low, (w, h), interpolation=cv2.INTER_CUBIC)
    img = (img * 200 + 25).clip(0, 255).astype(np.uint8)
    for _ in range(14):
        c = tuple(int(v) for v in rng.integers(0, 255, 3))
        p = (int(rng.integers(0, w)), int(rng.integers(0, h)))
        if rng.random() < 0.5:
            cv2.circle(img, p, int(rng.integers(15, 90)), c, -1, cv2.LINE_AA)
        else:
            q = (p[0] + int(rng.integers(20, 160)), p[1] + int(rng.integers(20, 120)))
            cv2.rectangle(img, p, q, c, -1)
    tex = rng.normal(0, 6, img.shape).astype(np.float32)
    tex = cv2.GaussianBlur(tex, (0, 0), 0.8) * 2.0
    return (img.astype(np.float32) + tex).clip(0, 255).astype(np.uint8)


def jpeg_roundtrip(rgb: np.ndarray, quality: int) -> np.ndarray:
    buf = io.BytesIO()
    Image.fromarray(rgb).save(buf, "JPEG", quality=quality)
    return np.asarray(Image.open(io.BytesIO(buf.getvalue())).convert("RGB"))


@pytest.fixture(scope="session")
def models_dir() -> Path:
    """Models used by @pytest.mark.models tests (git-ignored cache dir inside ai-worker)."""
    p = Path(os.environ.get("IMAGEPICKER_MODELS_DIR", AI_WORKER_ROOT / ".models"))
    p.mkdir(parents=True, exist_ok=True)
    return p


@pytest.fixture
def synth_files(tmp_path: Path) -> list[Path]:
    paths = []
    for i in range(4):
        p = tmp_path / f"img{i}.jpg"
        Image.fromarray(synth_image(i, (800 + 40 * i, 600))).save(p, "JPEG", quality=92)
        paths.append(p)
    return paths


@pytest.fixture(scope="session")
def portrait_path(tmp_path_factory: pytest.TempPathFactory) -> Path:
    """Public-domain portrait (Wikimedia Commons), downloaded at test time; skip when offline."""
    cache = Path(os.environ.get("IMAGEPICKER_TEST_CACHE", AI_WORKER_ROOT / ".models" / "_testimg"))
    cache.mkdir(parents=True, exist_ok=True)
    p = cache / "Albert_Einstein_Head.jpg"
    if not p.exists():
        url = "https://commons.wikimedia.org/wiki/Special:FilePath/Albert_Einstein_Head.jpg?width=1280"
        try:
            req = urllib.request.Request(url, headers={"User-Agent": "imagepicker-tests/0.1 (dev)"})
            with urllib.request.urlopen(req, timeout=30) as r:  # noqa: S310
                data = r.read()
            p.write_bytes(data)
        except Exception as e:  # noqa: BLE001
            pytest.skip(f"offline, cannot fetch test portrait: {e}")
    return p
