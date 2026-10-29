"""Face tone (cover choice) and the landmarks tri-state of the faces step (model-free)."""

from __future__ import annotations

import numpy as np
import pytest

from imagepicker_ai.steps.faces import FaceAnalyzer, face_tone

BOX = (0.25, 0.25, 0.5, 0.5)


def _img(v: int, side: int = 200) -> np.ndarray:
    return np.full((side, side, 3), v, np.uint8)


def test_face_tone_dark_flat_vs_lit_textured() -> None:
    luma, contrast = face_tone(_img(13), BOX)  # type: ignore[misc]
    assert luma == pytest.approx(13 / 255, abs=1e-3) and contrast == pytest.approx(0.0, abs=1e-6)
    rng = np.random.default_rng(0)
    lit = rng.integers(60, 200, (200, 200, 3), dtype=np.uint8)
    luma, contrast = face_tone(lit, BOX)  # type: ignore[misc]
    assert 0.4 < luma < 0.6 and contrast > 0.05


def test_face_tone_measures_the_inner_box_only() -> None:
    img = _img(0)
    # bright inside the central 70% of the box, black around it (hair / background)
    img[65:135, 65:135] = 200
    luma, _ = face_tone(img, BOX)  # type: ignore[misc]
    assert luma > 0.7
    assert face_tone(img, (0.5, 0.5, 0.004, 0.004)) is None  # under 4 px


class _Stub(FaceAnalyzer):
    def __init__(self, landmark: object) -> None:
        super().__init__("unused.onnx", None)
        self._lm_result = landmark

    def detect(self, rgb: np.ndarray) -> list[dict]:
        return [{"bbox": list(BOX), "det_score": 0.9}]

    def _landmark(self, rgb: np.ndarray, bbox: list[float]) -> dict:
        if isinstance(self._lm_result, Exception):
            raise self._lm_result
        return dict(self._lm_result)  # type: ignore[call-overload]


@pytest.mark.parametrize(
    ("ran", "result", "expected"),
    [
        (False, {}, None),  # no landmarker: unknown
        (True, {}, False),  # ran, found no human face (a dog, the back of a head)
        (True, {"landmarks": True, "eyes_open": 0.9, "smile": 0.1, "mouth_open": 0.0}, True),
        (True, RuntimeError("boom"), None),  # failed: unknown, not "no face"
    ],
)
def test_landmarks_tri_state(ran: bool, result: object, expected: bool | None) -> None:
    fa = _Stub(result)
    fa._landmarker_ok = ran
    f = fa.analyze(_img(120))[0]
    assert f["landmarks"] is expected
    assert f["luma"] == pytest.approx(120 / 255, abs=1e-3) and f["contrast"] == 0.0
    if expected is not True:
        assert f["eyes_open"] is None and f["blendshapes"] is None
