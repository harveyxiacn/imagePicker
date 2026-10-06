import hashlib

import pytest

from imagepicker_ai.errors import DownloadFailed, ModelUnavailable, OutOfMemory
from imagepicker_ai.hw import GpuInfo, HardwareInfo
from imagepicker_ai.models import Downloader, ModelManager, Registry, RegistryError
from imagepicker_ai.models.download import hf_endpoints
from imagepicker_ai.models.manager import compute_budget_mb

DOC07 = """
- id: siglip2-base
  task: [embed_image, embed_text, zero_shot]
  source: { hf: google/siglip2-base-patch16-224, modelscope: null }
  files: [model.onnx]
  backend: onnx
  tiers: [T1, T2, T3]
  vram_mb: 450
  size_mb: 375
  license: Apache-2.0
  sha256: "%s"
""" % ("ab" * 32)


def test_default_registry_parses():
    reg = Registry.load()
    assert len(reg) >= 5
    y = reg.get("yunet")
    assert y.backend == "opencv" and y.license == "MIT"
    assert y.primary.sha256 and len(y.primary.sha256) == 64
    s = reg.get("siglip2-base-fp16")
    assert s.hf_repo == "onnx-community/siglip2-base-patch16-224-ONNX"
    assert "embed_image" in s.task
    assert "T3" in s.tiers
    assert [m.id for m in reg.for_task("embed_text")][0] == "siglip2-base-text"
    # no non-commercial model is a default
    assert not any(m.noncommercial for m in reg)


def test_doc07_schema_form():
    reg = Registry.from_yaml(DOC07)
    m = reg.get("siglip2-base")
    assert m.task == ("embed_image", "embed_text", "zero_shot")
    assert m.vram_mb == 450 and m.size_mb == 375
    assert m.primary.path == "model.onnx"
    assert m.primary.sha256 == "ab" * 32  # model-level sha256 applies to the primary file
    assert m.to_public()["source"]["hf"] == "google/siglip2-base-patch16-224"


@pytest.mark.parametrize(
    "text",
    [
        "not: a list",
        "- {id: x}",
        "- {id: x, task: [a], backend: nope, files: [f], source: {hf: r}}",
        "- {id: x, task: [a], backend: onnx, source: {hf: r}}",
        "- {id: x, task: [a], backend: onnx, files: [f]}",
        "- {id: x, task: [a], backend: onnx, files: [{path: f, sha256: zz}], source: {hf: r}}",
        "- {id: x, task: [a], backend: onnx, files: [f], source: {hf: r}, tiers: [T9]}",
        "- {id: x, task: [a], backend: onnx, files: [f], source: {hf: r}}\n- {id: x, task: [a], backend: onnx, files: [f], source: {hf: r}}",
        "[unclosed",
    ],
)
def test_invalid_registry_rejected(text):
    with pytest.raises(RegistryError):
        Registry.from_yaml(text)


def test_url_file_without_hf_ok():
    reg = Registry.from_yaml(
        "- {id: u, task: [a], backend: mediapipe, files: [{path: f.task, url: 'http://x/f.task'}]}"
    )
    assert reg.get("u").primary.url == "http://x/f.task"


def test_endpoint_order(monkeypatch):
    monkeypatch.delenv("HF_ENDPOINT", raising=False)
    assert hf_endpoints() == ["https://huggingface.co", "https://hf-mirror.com"]
    monkeypatch.setenv("HF_ENDPOINT", "https://my.mirror/")
    assert hf_endpoints() == [
        "https://huggingface.co",
        "https://my.mirror",
        "https://hf-mirror.com",
    ]
    monkeypatch.setenv("HF_ENDPOINT", "https://hf-mirror.com")
    assert hf_endpoints() == ["https://huggingface.co", "https://hf-mirror.com"]


# --------------------------------------------------------------------------- downloader (mocked network)


def _spec(tmp_path, payload=b"hello-model", sha=True):
    s = hashlib.sha256(payload).hexdigest() if sha else None
    sha_line = f", sha256: {s}" if s else ""
    reg = Registry.from_yaml(
        f"- {{id: m, task: [t], backend: onnx, source: {{hf: org/repo}}, files: [{{path: sub/m.onnx, size: {len(payload)}{sha_line}}}, {{path: cfg.json}}]}}"
    )
    return reg.get("m")


def test_download_endpoint_fallback_atomic_and_manifest(tmp_path, monkeypatch):
    payload = b"hello-model"
    spec = _spec(tmp_path, payload)
    d = Downloader(tmp_path / "models", endpoints=["https://a", "https://b"])
    calls = []

    def fake_hf_get(self, spec, f, partial, endpoint, on_bytes):
        calls.append((endpoint, f.path))
        if endpoint == "https://a":
            raise ConnectionError("official unreachable")
        out = partial / f.path
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_bytes(payload if f.path.endswith("onnx") else b"{}")
        on_bytes(len(payload), len(payload))
        return out

    monkeypatch.setattr(Downloader, "_hf_get", fake_hf_get)
    events = []
    path = d.ensure(spec, events.append)
    assert path == tmp_path / "models" / "m" / "sub" / "m.onnx"
    assert path.read_bytes() == payload
    assert ("https://a", "sub/m.onnx") in calls and ("https://b", "sub/m.onnx") in calls
    assert d.is_installed(spec)
    assert not (tmp_path / "models" / ".partial" / "m").exists()
    assert events[-1]["phase"] == "done"
    assert all(
        b["bytes"] >= a["bytes"]
        for a, b in zip(events, events[1:], strict=False)
        if b["phase"] == "download"
    )
    # second call is a no-op
    calls.clear()
    d.ensure(spec)
    assert calls == []
    d.remove(spec)
    assert not d.is_installed(spec)


def test_download_sha_mismatch_fails_everywhere(tmp_path, monkeypatch):
    spec = _spec(tmp_path, b"expected-bytes")
    d = Downloader(tmp_path / "models", endpoints=["https://a", "https://b"])

    def bad(self, spec, f, partial, endpoint, on_bytes):
        out = partial / f.path
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_bytes(b"expected-bytes"[:-1] + b"X")  # same size, wrong content
        return out

    monkeypatch.setattr(Downloader, "_hf_get", bad)
    with pytest.raises(DownloadFailed) as ei:
        d.ensure(spec)
    assert len(ei.value.extra["errors"]) == 2
    assert not d.is_installed(spec)
    assert not (tmp_path / "models" / "m" / "sub" / "m.onnx").exists()


# --------------------------------------------------------------------------- ModelManager


REG = Registry.from_yaml(
    """
- {id: a, task: [t], backend: onnx, source: {hf: r}, files: [a.bin], vram_mb: 400, ram_mb: 100, resident: true}
- {id: b, task: [t], backend: onnx, source: {hf: r}, files: [b.bin], vram_mb: 400, ram_mb: 100, resident: false}
- {id: c, task: [t], backend: onnx, source: {hf: r}, files: [c.bin], vram_mb: 400, ram_mb: 100, resident: false}
- {id: huge, task: [t], backend: torch, source: {hf: r}, files: [h.bin], vram_mb: 5000, ram_mb: 100, exclusive_group: big}
- {id: huge2, task: [t], backend: torch, source: {hf: r}, files: [h.bin], vram_mb: 600, ram_mb: 100, exclusive_group: big}
"""
)


class Handle:
    def __init__(self, name):
        self.name = name
        self.closed = False

    def close(self):
        self.closed = True


def make_manager(tmp_path, budget=1000, monkeypatch=None):
    hw = HardwareInfo(
        "t",
        "cpu",
        4,
        8,
        16384,
        [GpuInfo("nvidia", "g", 8192)],
        device="cuda",
        ort_providers_available=["CUDAExecutionProvider"],
        tier="T2",
    )
    m = ModelManager(tmp_path, REG, hw, budget_mb=budget)
    m.downloader.is_installed = lambda spec: True  # pretend everything is installed
    return m


def test_budget_formula():
    hw = HardwareInfo("t", "cpu", 4, 8, 32768, [GpuInfo("nvidia", "g", 24000)], device="cuda")
    assert compute_budget_mb(hw) == int(24000 * 0.85) - 1536
    cpu = HardwareInfo("t", "cpu", 4, 8, 16384, device="cpu")
    assert compute_budget_mb(cpu) == 8192
    assert compute_budget_mb(hw, 123) == 123


def test_lru_eviction_prefers_non_resident_then_oldest(tmp_path):
    m = make_manager(tmp_path, budget=1000)
    made = {}

    def loader(spec, path):
        made[spec.id] = Handle(spec.id)
        return made[spec.id]

    m.acquire("a", loader)
    m.acquire("b", loader)
    assert m.used_mb() == 800
    m.acquire("c", loader)  # does not fit: evicts b (non-resident) rather than a (resident)
    assert made["b"].closed and not made["a"].closed
    assert {x["id"] for x in m.loaded()} == {"a", "c"}
    # re-acquire returns same handle, no reload
    assert m.acquire("a", loader) is made["a"]


def test_unload_and_oom(tmp_path):
    m = make_manager(tmp_path, budget=1000)
    m.acquire("a", lambda s, p: Handle("a"))
    assert m.unload("a") == ["a"]
    assert m.unload("a") == []
    with pytest.raises(OutOfMemory):
        m.acquire("huge", lambda s, p: Handle("h"))


def test_exclusive_group(tmp_path):
    m = make_manager(tmp_path, budget=6000)
    h1 = m.acquire("huge", lambda s, p: Handle("1"))
    m.acquire("huge2", lambda s, p: Handle("2"))
    assert h1.closed
    assert [x["id"] for x in m.loaded()] == ["huge2"]


def test_idle_sweep_only_non_resident(tmp_path):
    m = make_manager(tmp_path, budget=2000)
    m.acquire("a", lambda s, p: Handle("a"))
    m.acquire("b", lambda s, p: Handle("b"))
    import time

    time.sleep(0.05)
    assert m.sweep_idle(0.01) == ["b"]
    assert [x["id"] for x in m.loaded()] == ["a"]


def test_not_installed_raises(tmp_path):
    m = make_manager(tmp_path)
    m.downloader.is_installed = lambda spec: False
    with pytest.raises(ModelUnavailable):
        m.acquire("a", lambda s, p: Handle("a"))
    assert m.list_models()[0]["installed"] is False
