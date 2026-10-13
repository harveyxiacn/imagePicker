"""Structured licence fields of the model registry (docs/09 P1-17)."""

from __future__ import annotations

import pytest

from imagepicker_ai.hw import GpuInfo, HardwareInfo
from imagepicker_ai.models import ModelManager, Registry, RegistryError
from imagepicker_ai.models.registry import TRAINING_DATA_TERMS, USAGES

# Facts already recorded in docs/07 section 5 and docs/backlog.md (licence audit), as expected flags.
EXPECTED_UNKNOWN = {
    "nima-aesthetic": "research-only",  # AVA
    "nima-technical": "research-only",  # TID2013
    "skyseg-u2net": "undisclosed",
    "gfpgan-v1.4": "separate-terms",  # FFHQ
    "realesrgan-x2": "not-recorded",  # facefusion mirror, upstream BSD-3
    "realesrgan-x2-fp16": "not-recorded",
    "realesrgan-x4": "not-recorded",
    "realesrgan-x4-fp16": "not-recorded",
}


def test_every_entry_declares_license_info():
    reg = Registry.load()
    for s in reg:
        assert s.usage in USAGES, s.id
        assert s.training_data_terms in TRAINING_DATA_TERMS, s.id
        d = s.to_public()
        assert set(d["license_info"]) == {
            "weights",
            "training_data",
            "training_data_terms",
            "usage",
            "note",
        }
        assert d["usage"] == s.usage == d["license_info"]["usage"]
        assert d["license_info"]["weights"] == s.license


def test_entries_state_usage_explicitly():
    """`usage` must be written in registry.yaml for each entry, not left to the parser default."""
    import yaml

    from imagepicker_ai.models.registry import DEFAULT_REGISTRY

    for e in yaml.safe_load(DEFAULT_REGISTRY.read_text(encoding="utf-8")):
        info = e.get("license_info") or {}
        assert "usage" in info and "training_data_terms" in info, e["id"]


def test_recorded_audit_findings():
    reg = Registry.load()
    for mid, terms in EXPECTED_UNKNOWN.items():
        s = reg.get(mid)
        assert s.usage == "unknown", mid
        assert s.training_data_terms == terms, mid
        assert s.license_note, f"{mid}: say why it is unknown"
    assert reg.get("nima-aesthetic").training_data == "AVA (photographer-rated aesthetics)"
    assert "facefusion" in reg.get("realesrgan-x4").license_note
    assert "BSD-3" in reg.get("realesrgan-x4").license_note
    assert reg.get("auraface").usage == "commercial-ok"


def test_flags_are_consistent():
    for s in Registry.load():
        # terms with open questions can not be signed off as commercial-ok
        if s.training_data_terms in (
            "research-only",
            "non-commercial",
            "undisclosed",
            "separate-terms",
        ):
            assert s.usage != "commercial-ok", s.id
        # `noncommercial` (consent prompt) and usage agree
        assert s.noncommercial == (s.usage == "non-commercial"), s.id
        if s.usage != "commercial-ok":
            assert s.license_note, f"{s.id}: explain the flag"


def test_manager_listing_carries_the_flags(tmp_path):
    hw = HardwareInfo("t", "cpu", 4, 8, 16384, [GpuInfo("nvidia", "g", 8192)], tier="T2")
    listing = {m["id"]: m for m in ModelManager(tmp_path, Registry.load(), hw).list_models()}
    assert listing["skyseg-u2net"]["usage"] == "unknown"
    assert listing["skyseg-u2net"]["license_info"]["training_data_terms"] == "undisclosed"
    assert listing["yunet"]["usage"] == "commercial-ok"


@pytest.mark.parametrize(
    "extra",
    [
        "license_info: {usage: free}",
        "license_info: {training_data_terms: whatever}",
        "license_info: {surprise: 1}",
        "license_info: nope",
    ],
)
def test_bad_license_info_rejected(extra):
    with pytest.raises(RegistryError):
        Registry.from_yaml(
            f"- {{id: x, task: [a], backend: onnx, files: [f], source: {{hf: r}}, {extra}}}"
        )


def test_license_info_is_optional_and_defaults_to_unknown():
    s = Registry.from_yaml("- {id: x, task: [a], backend: onnx, files: [f], source: {hf: r}}").get(
        "x"
    )
    assert s.usage == "unknown" and s.training_data_terms == "not-recorded"
