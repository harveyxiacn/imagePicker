"""64-bit DCT perceptual hash (pHash).

Algorithm: grayscale -> area-resample to 32x32 -> 2-D DCT-II -> keep the 8x8 low-frequency
block -> bit_i = (coef_i > median(block)).  Bits are packed row-major, MSB first, into 16 hex chars.
Hamming distance <= ~10 means "same picture" (resize / recompression gives typically 0-4).
"""

from __future__ import annotations

import cv2
import numpy as np

HASH_SIZE = 8
_DCT_SIZE = 32


def phash_bits(rgb: np.ndarray) -> np.ndarray:
    """Return the 64 hash bits as a bool array."""
    gray = cv2.cvtColor(rgb, cv2.COLOR_RGB2GRAY) if rgb.ndim == 3 else rgb
    small = cv2.resize(gray, (_DCT_SIZE, _DCT_SIZE), interpolation=cv2.INTER_AREA).astype(
        np.float32
    )
    low = cv2.dct(small)[:HASH_SIZE, :HASH_SIZE]
    # The DC term dominates the median otherwise; use the median of the other 63 coefficients.
    med = np.median(low.ravel()[1:])
    return (low > med).ravel()


def phash_hex(rgb: np.ndarray) -> str:
    bits = phash_bits(rgb)
    v = 0
    for b in bits:
        v = (v << 1) | int(b)
    return f"{v:016x}"


def hamming(a: str, b: str) -> int:
    return (int(a, 16) ^ int(b, 16)).bit_count()
