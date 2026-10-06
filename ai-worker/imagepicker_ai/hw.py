"""Hardware detection and tier mapping (doc 03 §8.1, doc 02 §6).

Tiers (doc 03 §8.1):
  T0 CPU            no usable GPU
  T1 entry GPU      Apple M-series (unified memory >= 8 GB), 4-6 GB (<8) dedicated GPU,
                    Intel/AMD integrated GPU (DirectML)
  T2 mid GPU        8-12 GB (<16) dedicated GPU, Apple M Pro/Max/Ultra
  T3 high GPU       >= 16 GB dedicated GPU (e.g. RTX 4090 24 GB)
"""

from __future__ import annotations

import logging
import os
import platform
import re
import shutil
import subprocess
from dataclasses import asdict, dataclass, field
from typing import Any

import psutil

log = logging.getLogger(__name__)

# Execution-provider preference, best first. TensorRT is intentionally not auto-selected
# (long engine builds on first run); users can opt in through the providers override.
EP_PREFERENCE = [
    "CUDAExecutionProvider",
    "ROCMExecutionProvider",
    "MIGraphXExecutionProvider",
    "CoreMLExecutionProvider",
    "DmlExecutionProvider",
    "CPUExecutionProvider",
]


@dataclass
class GpuInfo:
    vendor: str  # nvidia | amd | intel | apple | other
    name: str
    vram_mb: int | None = None  # dedicated VRAM (or unified memory for Apple)
    driver: str | None = None
    unified_memory: bool = False


@dataclass
class HardwareInfo:
    os: str
    cpu_name: str
    cpu_cores_physical: int
    cpu_cores_logical: int
    ram_mb: int
    gpus: list[GpuInfo] = field(default_factory=list)
    apple_mps: bool = False
    ort_providers_available: list[str] = field(default_factory=list)
    torch_cuda: bool = False
    tier: str = "T0"
    tier_reason: str = ""
    providers: list[str] = field(default_factory=lambda: ["CPUExecutionProvider"])
    device: str = "cpu"  # cuda | rocm | mps | dml | cpu  (best accelerator actually usable)

    def to_dict(self) -> dict[str, Any]:
        return asdict(self)

    @property
    def primary_gpu(self) -> GpuInfo | None:
        # largest-VRAM GPU wins
        if not self.gpus:
            return None
        return max(self.gpus, key=lambda g: g.vram_mb or 0)


# ---------------------------------------------------------------- tier mapping (pure)


def _tier_for_vram(vram_mb: int) -> str:
    if vram_mb >= 16 * 1024 - 512:  # 16 GB cards report ~16 000-16 300 MiB
        return "T3"
    if vram_mb >= 8 * 1024 - 512:
        return "T2"
    if vram_mb >= 4 * 1024 - 256:
        return "T1"
    return "T0"


def compute_tier(hw: HardwareInfo) -> tuple[str, str]:
    """Map detected hardware to (tier, reason). Pure function, no I/O."""
    discrete = [g for g in hw.gpus if g.vendor in ("nvidia", "amd") and not g.unified_memory]
    best: tuple[str, str] = ("T0", "no usable GPU, CPU only")

    def consider(tier: str, reason: str) -> None:
        nonlocal best
        if (
            tier >= best[0]
        ):  # "T3" > "T2" lexicographically; later/equal candidates replace the reason
            best = (tier, reason)

    for g in discrete:
        if g.vram_mb:
            # AMD/NVIDIA discrete cards only count if an accelerated runtime can really use them.
            usable = (
                (g.vendor == "nvidia" and (hw.torch_cuda or _has(hw, "CUDAExecutionProvider")))
                or (
                    g.vendor == "amd"
                    and (
                        _has(
                            hw,
                            "ROCMExecutionProvider",
                            "MIGraphXExecutionProvider",
                            "DmlExecutionProvider",
                        )
                    )
                )
                or (_has(hw, "DmlExecutionProvider"))
            )
            if usable:
                consider(_tier_for_vram(g.vram_mb), f"{g.name} {g.vram_mb // 1024} GB")
            else:
                consider(
                    "T0", f"{g.name} detected but no GPU runtime available (install the cuda extra)"
                )

    if hw.apple_mps or any(g.vendor == "apple" for g in hw.gpus):
        mem_gb = hw.ram_mb / 1024
        name = next((g.name for g in hw.gpus if g.vendor == "apple"), hw.cpu_name)
        if re.search(r"\b(pro|max|ultra)\b", name, re.I) and mem_gb >= 16:
            consider("T2", f"{name} unified memory {mem_gb:.0f} GB")
        elif mem_gb >= 8 - 0.5:
            consider("T1", f"{name} unified memory {mem_gb:.0f} GB")

    # Integrated / non-NVIDIA GPU reachable through DirectML
    if best[0] == "T0" and _has(hw, "DmlExecutionProvider") and hw.gpus:
        consider("T1", f"{hw.gpus[0].name} via DirectML")

    if best[0] in ("T1", "T2", "T3") and hw.ram_mb < 6 * 1024:
        return "T0", f"{best[1]} but only {hw.ram_mb // 1024} GB system RAM"
    return best


def _has(hw: HardwareInfo, *eps: str) -> bool:
    return any(e in hw.ort_providers_available for e in eps)


def select_providers(available: list[str], device_pref: str = "auto") -> list[str]:
    """Ordered ONNX Runtime providers to request. `device_pref='cpu'` forces CPU."""
    if device_pref == "cpu":
        return ["CPUExecutionProvider"]
    chosen = [e for e in EP_PREFERENCE if e in available and e != "CPUExecutionProvider"]
    chosen.append("CPUExecutionProvider")
    return chosen


def _device_of(providers: list[str], apple_mps: bool) -> str:
    first = providers[0]
    return {
        "CUDAExecutionProvider": "cuda",
        "ROCMExecutionProvider": "rocm",
        "MIGraphXExecutionProvider": "rocm",
        "CoreMLExecutionProvider": "mps",
        "DmlExecutionProvider": "dml",
    }.get(first, "mps" if apple_mps and first != "CPUExecutionProvider" else "cpu")


# ---------------------------------------------------------------- detection (I/O)


def _nvidia_smi() -> list[GpuInfo]:
    exe = shutil.which("nvidia-smi")
    if exe is None and os.name == "nt":
        cand = r"C:\Windows\System32\nvidia-smi.exe"
        exe = cand if os.path.exists(cand) else None
    if exe:
        try:
            out = subprocess.run(
                [
                    exe,
                    "--query-gpu=name,memory.total,driver_version",
                    "--format=csv,noheader,nounits",
                ],
                capture_output=True,
                text=True,
                timeout=10,
                check=True,
                creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
            ).stdout
            gpus = []
            for line in out.strip().splitlines():
                parts = [p.strip() for p in line.split(",")]
                if len(parts) >= 3:
                    gpus.append(GpuInfo("nvidia", parts[0], int(float(parts[1])), parts[2]))
            if gpus:
                return gpus
        except Exception as e:  # noqa: BLE001
            log.debug("nvidia-smi failed: %s", e)
    try:  # optional pynvml fallback
        import pynvml  # type: ignore[import-not-found]

        pynvml.nvmlInit()
        try:
            drv = pynvml.nvmlSystemGetDriverVersion()
            drv = drv.decode() if isinstance(drv, bytes) else str(drv)
            gpus = []
            for i in range(pynvml.nvmlDeviceGetCount()):
                h = pynvml.nvmlDeviceGetHandleByIndex(i)
                name = pynvml.nvmlDeviceGetName(h)
                name = name.decode() if isinstance(name, bytes) else str(name)
                gpus.append(
                    GpuInfo(
                        "nvidia", name, pynvml.nvmlDeviceGetMemoryInfo(h).total // (1 << 20), drv
                    )
                )
            return gpus
        finally:
            pynvml.nvmlShutdown()
    except Exception:  # noqa: BLE001
        return []


def nvidia_memory_usage() -> dict[str, int] | None:
    """Live (total_mb, used_mb, free_mb) of GPU 0 via nvidia-smi, or None."""
    exe = shutil.which("nvidia-smi")
    if not exe:
        return None
    try:
        out = (
            subprocess.run(
                [
                    exe,
                    "--query-gpu=memory.total,memory.used,memory.free",
                    "--format=csv,noheader,nounits",
                ],
                capture_output=True,
                text=True,
                timeout=5,
                check=True,
                creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
            )
            .stdout.strip()
            .splitlines()[0]
        )
        t, u, f = (int(float(x)) for x in out.split(","))
        return {"total_mb": t, "used_mb": u, "free_mb": f}
    except Exception:  # noqa: BLE001
        return None


def _cpu_name() -> str:
    name = platform.processor() or platform.machine()
    if os.name == "nt":
        try:
            import winreg

            with winreg.OpenKey(
                winreg.HKEY_LOCAL_MACHINE, r"HARDWARE\DESCRIPTION\System\CentralProcessor\0"
            ) as k:
                name = winreg.QueryValueEx(k, "ProcessorNameString")[0].strip()
        except Exception:  # noqa: BLE001
            pass
    elif platform.system() == "Darwin":
        try:
            name = (
                subprocess.run(
                    ["sysctl", "-n", "machdep.cpu.brand_string"],
                    capture_output=True,
                    text=True,
                    timeout=5,
                ).stdout.strip()
                or name
            )
        except Exception:  # noqa: BLE001
            pass
    return name


def _ort_providers() -> list[str]:
    try:
        import onnxruntime as ort

        return list(ort.get_available_providers())
    except Exception:  # noqa: BLE001
        return []


def _torch_cuda() -> bool:
    # Do not import torch just to ask (it is heavy); only if already imported by someone.
    import sys

    t = sys.modules.get("torch")
    if t is None:
        return False
    try:
        return bool(t.cuda.is_available())
    except Exception:  # noqa: BLE001
        return False


def _windows_gpu_names() -> list[str]:
    if os.name != "nt":
        return []
    try:
        out = subprocess.run(
            [
                "powershell",
                "-NoProfile",
                "-Command",
                "(Get-CimInstance Win32_VideoController).Name",
            ],
            capture_output=True,
            text=True,
            timeout=10,
            creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
        ).stdout
        return [ln.strip() for ln in out.splitlines() if ln.strip()]
    except Exception:  # noqa: BLE001
        return []


def detect(device_pref: str = "auto") -> HardwareInfo:
    """Probe the machine. `device_pref='cpu'` keeps tier detection but forces CPU providers."""
    vm = psutil.virtual_memory()
    sysname = platform.system()
    hw = HardwareInfo(
        os=f"{sysname} {platform.release()}",
        cpu_name=_cpu_name(),
        cpu_cores_physical=psutil.cpu_count(logical=False) or os.cpu_count() or 1,
        cpu_cores_logical=psutil.cpu_count(logical=True) or os.cpu_count() or 1,
        ram_mb=int(vm.total // (1 << 20)),
    )
    hw.gpus = _nvidia_smi()
    hw.ort_providers_available = _ort_providers()
    hw.torch_cuda = _torch_cuda()

    if sysname == "Darwin" and platform.machine() == "arm64":
        hw.apple_mps = True
        hw.gpus.append(GpuInfo("apple", hw.cpu_name, hw.ram_mb, unified_memory=True))
    elif not hw.gpus:
        for n in _windows_gpu_names():
            low = n.lower()
            if "nvidia" in low:
                continue
            vendor = (
                "amd"
                if ("amd" in low or "radeon" in low)
                else "intel"
                if "intel" in low
                else "other"
            )
            if "microsoft basic" in low or "remote" in low:
                continue
            hw.gpus.append(GpuInfo(vendor, n))

    hw.tier, hw.tier_reason = compute_tier(hw)
    hw.providers = select_providers(hw.ort_providers_available, device_pref)
    hw.device = _device_of(hw.providers, hw.apple_mps)
    return hw
