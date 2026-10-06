import cv2
import numpy as np

from imagepicker_ai.steps.phash import hamming, phash_hex

from .conftest import jpeg_roundtrip, synth_image


def test_format():
    h = phash_hex(synth_image(1))
    assert len(h) == 16
    int(h, 16)


def test_deterministic():
    img = synth_image(2)
    assert phash_hex(img) == phash_hex(img.copy())


def test_invariant_to_resize_and_jpeg():
    img = synth_image(3, (1024, 768))
    base = phash_hex(img)
    small = cv2.resize(img, (400, 300), interpolation=cv2.INTER_AREA)
    assert hamming(base, phash_hex(small)) <= 6
    assert hamming(base, phash_hex(jpeg_roundtrip(img, 40))) <= 6
    assert hamming(base, phash_hex(jpeg_roundtrip(small, 60))) <= 8
    bright = np.clip(img.astype(np.int16) + 12, 0, 255).astype(np.uint8)
    assert hamming(base, phash_hex(bright)) <= 6


def test_different_images_are_far():
    hashes = [phash_hex(synth_image(s, (1024, 768))) for s in range(10, 20)]
    dists = [hamming(a, b) for i, a in enumerate(hashes) for b in hashes[i + 1 :]]
    assert min(dists) >= 12
    assert np.mean(dists) > 24


def test_hamming():
    assert hamming("0000000000000000", "ffffffffffffffff") == 64
    assert hamming("00000000000000ff", "0000000000000000") == 8
