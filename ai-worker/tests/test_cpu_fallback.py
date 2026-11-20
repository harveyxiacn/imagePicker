"""The OOM -> CPU fallback is a cool-down, not a permanent switch (fake clock, fake handles)."""

import numpy as np
import pytest

from imagepicker_ai.cpu_fallback import OOM_COOLDOWN_MAX_S, OOM_COOLDOWN_S, CpuFallback
from imagepicker_ai.errors import OutOfMemory
from imagepicker_ai.hw import GpuInfo, HardwareInfo
from imagepicker_ai.masks.generator import MaskGenerator
from imagepicker_ai.steps.embed import Embedder

CPU = ["CPUExecutionProvider"]


class Clock:
    def __init__(self) -> None:
        self.t = 1000.0

    def __call__(self) -> float:
        return self.t


class FakeManager:
    """Just what the fallback paths touch: unload() and is_installed()."""

    def __init__(self) -> None:
        self.unloads: list[bool] = []

    def unload(self, model_id=None, *, only_idle=False):
        self.unloads.append(only_idle)
        return []

    def is_installed(self, model_id: str) -> bool:
        return True


def hw(device: str) -> HardwareInfo:
    gpus = [GpuInfo("nvidia", "g", 8192)] if device == "cuda" else []
    return HardwareInfo(
        "t",
        "cpu",
        4,
        8,
        16384,
        gpus,
        device=device,
        ort_providers_available=["CUDAExecutionProvider"] if device == "cuda" else [],
        providers=["CUDAExecutionProvider", "CPUExecutionProvider"] if device == "cuda" else CPU,
        tier="T2",
    )


GPU = hw("cuda").providers
assert GPU != CPU


def oom() -> RuntimeError:
    return RuntimeError("CUDA failure: out of memory")


def test_cooldown_expires_then_rearms_longer():
    clock, freed = Clock(), []
    fb = CpuFallback(False, on_retry_gpu=lambda: freed.append(clock()), clock=clock)
    assert not fb.active
    assert fb.trip() == OOM_COOLDOWN_S
    assert fb.active
    clock.t += OOM_COOLDOWN_S - 1
    assert fb.active and not freed
    clock.t += 2
    assert not fb.active and len(freed) == 1  # GPU again; the CPU-loaded handles are dropped once
    assert not fb.active and len(freed) == 1
    assert fb.trip() == 2 * OOM_COOLDOWN_S  # repeated OOM: longer cool-down
    clock.t += 2 * OOM_COOLDOWN_S + 1
    assert not fb.active
    for _ in range(10):  # a GPU that is permanently too small settles at the cap
        length = fb.trip()
        clock.t += length + 1
        assert not fb.active
    assert length == OOM_COOLDOWN_MAX_S


def test_a_long_quiet_period_resets_the_backoff():
    clock = Clock()
    fb = CpuFallback(False, clock=clock)
    fb.trip()
    clock.t += OOM_COOLDOWN_S + 1
    assert not fb.active
    fb.trip()
    clock.t += 2 * OOM_COOLDOWN_S + 1
    assert not fb.active
    clock.t += OOM_COOLDOWN_MAX_S + 1  # an hour without trouble
    assert fb.trip() == OOM_COOLDOWN_S


def test_cpu_only_hardware_never_switches():
    clock, freed = Clock(), []
    fb = CpuFallback(True, on_retry_gpu=lambda: freed.append(1), clock=clock)
    assert fb.active and fb.trip() == 0.0
    clock.t += 10 * OOM_COOLDOWN_MAX_S
    assert fb.active and not freed


def test_mask_generator_returns_to_the_gpu_after_the_cooldown(monkeypatch):
    clock, mgr = Clock(), FakeManager()
    g = MaskGenerator(mgr, hw("cuda"), clock=clock)
    try:
        calls: list[list[str]] = []
        fail = {"n": 1}

        def compute(photo, target, bbox):
            calls.append(g._providers())
            if fail["n"]:
                fail["n"] -= 1
                raise oom()
            return np.zeros((2, 2), np.float32), "m"

        monkeypatch.setattr(g, "_compute", compute)
        assert g._providers() == GPU
        g._run_target(None, "subject", None)  # OOM on the GPU, retried on the CPU
        assert calls == [GPU, CPU] and mgr.unloads == [True]  # the OOM path spares busy models
        assert g.force_cpu and g._providers() == CPU
        clock.t += OOM_COOLDOWN_S + 1
        assert not g.force_cpu and g._providers() == GPU  # next request tries the GPU again
        assert mgr.unloads == [True, True]  # ... after the CPU-loaded handles were dropped
        fail["n"] = 1
        calls.clear()
        g._run_target(None, "subject", None)  # a second OOM goes back to the CPU
        assert calls == [GPU, CPU] and g._providers() == CPU
        clock.t += OOM_COOLDOWN_S + 1  # ... for longer than the first time
        assert g._providers() == CPU
        clock.t += OOM_COOLDOWN_S
        assert g._providers() == GPU
    finally:
        g.shutdown()


def test_mask_generator_on_cpu_only_hardware_stays_cpu(monkeypatch):
    clock = Clock()
    g = MaskGenerator(FakeManager(), hw("cpu"), clock=clock)
    try:

        def compute(photo, target, bbox):
            raise oom()

        monkeypatch.setattr(g, "_compute", compute)
        with pytest.raises(OutOfMemory):
            g._run_target(None, "subject", None)
        clock.t += 100 * OOM_COOLDOWN_MAX_S
        assert g.force_cpu and g._providers() == CPU
    finally:
        g.shutdown()


class FakeEmbedModel:
    providers = GPU

    def __init__(self, emb: Embedder, log: list[str]) -> None:
        self.emb, self.log = emb, log

    def run(self, x: np.ndarray) -> np.ndarray:
        self.log.append("cpu" if self.emb.force_cpu else "gpu")
        if not self.emb.force_cpu:
            raise oom()
        return np.ones((len(x), 768), np.float32)


def test_embedder_falls_back_and_returns_to_the_gpu(monkeypatch):
    clock, mgr, log = Clock(), FakeManager(), []
    emb = Embedder(mgr, hw("cuda"), batch_size=1, backend="onnx", clock=clock)
    model = FakeEmbedModel(emb, log)
    mgr.acquire = lambda mid, loader, cost_mb=None: model  # type: ignore[attr-defined]
    from contextlib import nullcontext

    mgr.pin = lambda mid: nullcontext()  # type: ignore[attr-defined]
    imgs = [np.zeros((224, 224, 3), np.uint8)]
    assert emb.embed_prepared(imgs).shape == (1, 768)
    assert log == ["gpu", "cpu"] and emb.force_cpu and emb._providers() == CPU
    clock.t += OOM_COOLDOWN_S + 1
    assert not emb.force_cpu and emb._providers() == GPU
    log.clear()
    emb.embed_prepared(imgs)  # tries the GPU again, OOMs again, back to the CPU
    # the batch size was cut to 8 on the CPU: it halves again on the GPU before the final fallback
    assert log[0] == "gpu" and log[-1] == "cpu" and emb.force_cpu


def test_embedder_on_cpu_only_hardware_never_trips():
    clock = Clock()
    emb = Embedder(FakeManager(), hw("cpu"), backend="onnx", clock=clock)
    clock.t += 100 * OOM_COOLDOWN_MAX_S
    assert emb.force_cpu and emb._providers() == CPU
