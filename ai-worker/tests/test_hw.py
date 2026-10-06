import pytest

from imagepicker_ai import hw as hwmod
from imagepicker_ai.hw import GpuInfo, HardwareInfo, compute_tier, select_providers

CUDA = ["CUDAExecutionProvider", "CPUExecutionProvider"]
CPU = ["CPUExecutionProvider"]
DML = ["DmlExecutionProvider", "CPUExecutionProvider"]


def make(gpus=None, ram_gb=16, providers=CPU, apple=False, cpu="Test CPU") -> HardwareInfo:
    return HardwareInfo(
        os="test",
        cpu_name=cpu,
        cpu_cores_physical=8,
        cpu_cores_logical=16,
        ram_mb=ram_gb * 1024,
        gpus=gpus or [],
        apple_mps=apple,
        ort_providers_available=providers,
    )


@pytest.mark.parametrize(
    ("vram_gb", "tier"),
    [(24, "T3"), (16, "T3"), (12, "T2"), (10, "T2"), (8, "T2"), (6, "T1"), (4, "T1"), (2, "T0")],
)
def test_nvidia_vram_tiers(vram_gb, tier):
    hw = make([GpuInfo("nvidia", "RTX", vram_gb * 1024)], providers=CUDA)
    assert compute_tier(hw)[0] == tier


def test_rtx4090_is_t3():
    hw = make([GpuInfo("nvidia", "NVIDIA GeForce RTX 4090", 24564)], ram_gb=64, providers=CUDA)
    tier, reason = compute_tier(hw)
    assert tier == "T3"
    assert "4090" in reason


def test_16gb_card_reporting_slightly_less_is_t3():
    assert compute_tier(make([GpuInfo("nvidia", "RTX 4080", 16077)], providers=CUDA))[0] == "T3"


def test_8gb_card_reporting_slightly_less_is_t2():
    assert (
        compute_tier(make([GpuInfo("nvidia", "RTX 3070", 8192 - 256)], providers=CUDA))[0] == "T2"
    )


def test_nvidia_without_runtime_is_t0():
    hw = make([GpuInfo("nvidia", "RTX 4090", 24564)], providers=CPU)
    tier, reason = compute_tier(hw)
    assert tier == "T0"
    assert "runtime" in reason


def test_cpu_only_is_t0():
    assert compute_tier(make())[0] == "T0"


def test_apple_silicon():
    base = make(
        [GpuInfo("apple", "Apple M2", 16 * 1024, unified_memory=True)],
        ram_gb=16,
        apple=True,
        providers=["CoreMLExecutionProvider", "CPUExecutionProvider"],
        cpu="Apple M2",
    )
    assert compute_tier(base)[0] == "T1"
    pro = make(
        [GpuInfo("apple", "Apple M3 Pro", 36 * 1024, unified_memory=True)],
        ram_gb=36,
        apple=True,
        providers=["CoreMLExecutionProvider", "CPUExecutionProvider"],
        cpu="Apple M3 Pro",
    )
    assert compute_tier(pro)[0] == "T2"
    tiny = make(
        [GpuInfo("apple", "Apple M1", 4096, unified_memory=True)],
        ram_gb=4,
        apple=True,
        providers=["CoreMLExecutionProvider", "CPUExecutionProvider"],
        cpu="Apple M1",
    )
    assert compute_tier(tiny)[0] == "T0"


def test_integrated_gpu_via_directml_is_t1():
    hw = make([GpuInfo("intel", "Intel(R) Iris(R) Xe Graphics")], providers=DML)
    assert compute_tier(hw)[0] == "T1"


def test_low_ram_caps_to_t0():
    hw = make([GpuInfo("nvidia", "RTX 4060", 8192)], ram_gb=4, providers=CUDA)
    assert compute_tier(hw)[0] == "T0"


def test_best_gpu_wins():
    hw = make(
        [GpuInfo("nvidia", "RTX 3050", 4096), GpuInfo("nvidia", "RTX 4090", 24564)], providers=CUDA
    )
    assert compute_tier(hw)[0] == "T3"


def test_provider_selection_order():
    avail = ["TensorrtExecutionProvider", "CUDAExecutionProvider", "CPUExecutionProvider"]
    assert select_providers(avail) == ["CUDAExecutionProvider", "CPUExecutionProvider"]
    assert select_providers(avail, "cpu") == ["CPUExecutionProvider"]
    assert select_providers(DML) == ["DmlExecutionProvider", "CPUExecutionProvider"]
    assert (
        select_providers(["CoreMLExecutionProvider", "CPUExecutionProvider"])[0]
        == "CoreMLExecutionProvider"
    )
    assert (
        select_providers(["ROCMExecutionProvider", "CPUExecutionProvider"])[0]
        == "ROCMExecutionProvider"
    )
    assert select_providers([]) == ["CPUExecutionProvider"]


def test_detect_with_mocked_machine(monkeypatch):
    monkeypatch.setattr(
        hwmod, "_nvidia_smi", lambda: [GpuInfo("nvidia", "NVIDIA GeForce RTX 4090", 24564, "616.0")]
    )
    monkeypatch.setattr(hwmod, "_ort_providers", lambda: CUDA)
    monkeypatch.setattr(hwmod, "_cpu_name", lambda: "Fake CPU")
    hw = hwmod.detect()
    assert hw.tier == "T3"
    assert hw.providers == CUDA
    assert hw.device == "cuda"
    assert hw.to_dict()["gpus"][0]["vram_mb"] == 24564
    forced = hwmod.detect("cpu")
    assert forced.providers == CPU
    assert forced.device == "cpu"
    assert forced.tier == "T3"  # hardware tier is unchanged, only the providers are forced


def test_detect_runs_for_real():
    hw = hwmod.detect()
    assert hw.cpu_cores_logical >= 1
    assert hw.ram_mb > 0
    assert hw.tier in ("T0", "T1", "T2", "T3")
    assert hw.providers[-1] == "CPUExecutionProvider"
