"""Model registry parsing/validation (schema: doc 07 §1)."""

from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

import yaml

DEFAULT_REGISTRY = Path(__file__).with_name("registry.yaml")
BACKENDS = {"onnx", "torch", "mediapipe", "opencv"}
TIERS = ("T0", "T1", "T2", "T3")


class RegistryError(ValueError):
    pass


@dataclass(frozen=True)
class ModelFile:
    path: str
    sha256: str | None = None
    size: int | None = None
    url: str | None = None


@dataclass(frozen=True)
class ModelSpec:
    id: str
    task: tuple[str, ...]
    files: tuple[ModelFile, ...]
    backend: str
    hf_repo: str | None = None
    hf_revision: str | None = None
    modelscope: str | None = None
    origin: str | None = None
    precision: str | None = None
    tiers: tuple[str, ...] = ()
    vram_mb: int = 0
    ram_mb: int = 0
    size_mb: float = 0.0
    license: str | None = None
    noncommercial: bool = False
    resident: bool = False
    exclusive_group: str | None = None
    optional: bool = False
    load_time_s: float | None = None
    extra: dict[str, Any] = field(default_factory=dict, compare=False)

    @property
    def primary(self) -> ModelFile:
        return self.files[0]

    def to_public(self) -> dict[str, Any]:
        return {
            "id": self.id,
            "task": list(self.task),
            "backend": self.backend,
            "precision": self.precision,
            "tiers": list(self.tiers),
            "vram_mb": self.vram_mb,
            "ram_mb": self.ram_mb,
            "size_mb": self.size_mb,
            "license": self.license,
            "noncommercial": self.noncommercial,
            "resident": self.resident,
            "optional": self.optional,
            "source": {"hf": self.hf_repo, "modelscope": self.modelscope, "origin": self.origin},
        }


def _as_list(v: Any) -> list[Any]:
    if v is None:
        return []
    return list(v) if isinstance(v, (list, tuple)) else [v]


def _parse_entry(e: Any, idx: int) -> ModelSpec:
    if not isinstance(e, dict):
        raise RegistryError(f"entry #{idx}: expected mapping, got {type(e).__name__}")
    mid = e.get("id")
    if not isinstance(mid, str) or not mid:
        raise RegistryError(f"entry #{idx}: missing id")
    where = f"model {mid!r}"

    task = [str(t) for t in _as_list(e.get("task"))]
    if not task:
        raise RegistryError(f"{where}: task must be a non-empty list")

    backend = e.get("backend")
    if backend not in BACKENDS:
        raise RegistryError(f"{where}: backend must be one of {sorted(BACKENDS)}, got {backend!r}")

    source = e.get("source") or {}
    if not isinstance(source, dict):
        raise RegistryError(f"{where}: source must be a mapping")

    files_raw = e.get("files")
    if not files_raw:
        # doc 07 minimal form: single file named by `file`, or whole repo unsupported
        raise RegistryError(f"{where}: files must list at least one file")
    files: list[ModelFile] = []
    for f in files_raw:
        if isinstance(f, str):
            f = {"path": f}
        if not isinstance(f, dict) or not f.get("path"):
            raise RegistryError(f"{where}: bad file entry {f!r}")
        sha = f.get("sha256")
        if sha is not None and (
            not isinstance(sha, str)
            or len(sha) != 64
            or any(c not in "0123456789abcdef" for c in sha.lower())
        ):
            raise RegistryError(f"{where}: file {f['path']!r} has invalid sha256")
        mf = ModelFile(
            path=str(f["path"]),
            sha256=sha.lower() if sha else None,
            size=f.get("size"),
            url=f.get("url"),
        )
        if mf.url is None and not source.get("hf"):
            raise RegistryError(f"{where}: file {mf.path!r} needs either source.hf or its own url")
        files.append(mf)
    # doc 07 form: a model-level sha256 applies to the primary file
    top_sha = e.get("sha256")
    if isinstance(top_sha, str) and files[0].sha256 is None:
        files[0] = ModelFile(files[0].path, top_sha.lower(), files[0].size, files[0].url)

    tiers = [str(t) for t in _as_list(e.get("tiers"))]
    for t in tiers:
        if t not in TIERS:
            raise RegistryError(f"{where}: unknown tier {t!r}")

    known = {
        "id",
        "task",
        "source",
        "files",
        "backend",
        "precision",
        "tiers",
        "vram_mb",
        "ram_mb",
        "size_mb",
        "license",
        "noncommercial",
        "resident",
        "exclusive_group",
        "optional",
        "load_time",
        "sha256",
    }
    return ModelSpec(
        id=mid,
        task=tuple(task),
        files=tuple(files),
        backend=backend,
        hf_repo=source.get("hf"),
        hf_revision=source.get("revision"),
        modelscope=source.get("modelscope"),
        origin=source.get("origin"),
        precision=e.get("precision"),
        tiers=tuple(tiers),
        vram_mb=int(e.get("vram_mb") or 0),
        ram_mb=int(e.get("ram_mb") or 0),
        size_mb=float(e.get("size_mb") or 0),
        license=e.get("license"),
        noncommercial=bool(e.get("noncommercial", False)),
        resident=bool(e.get("resident", False)),
        exclusive_group=e.get("exclusive_group"),
        optional=bool(e.get("optional", False)),
        load_time_s=e.get("load_time"),
        extra={k: v for k, v in e.items() if k not in known},
    )


class Registry:
    def __init__(self, specs: list[ModelSpec]):
        self._by_id: dict[str, ModelSpec] = {}
        for s in specs:
            if s.id in self._by_id:
                raise RegistryError(f"duplicate model id {s.id!r}")
            self._by_id[s.id] = s

    def __contains__(self, mid: str) -> bool:
        return mid in self._by_id

    def __iter__(self):
        return iter(self._by_id.values())

    def __len__(self) -> int:
        return len(self._by_id)

    def get(self, mid: str) -> ModelSpec:
        try:
            return self._by_id[mid]
        except KeyError:
            raise KeyError(f"unknown model id {mid!r}") from None

    def for_task(self, task: str) -> list[ModelSpec]:
        return [s for s in self if task in s.task]

    @classmethod
    def from_yaml(cls, text: str) -> Registry:
        try:
            data = yaml.safe_load(text)
        except yaml.YAMLError as e:
            raise RegistryError(f"invalid YAML: {e}") from e
        if isinstance(data, dict):
            data = data.get("models")
        if not isinstance(data, list):
            raise RegistryError("registry must be a list of models (or {models: [...]})")
        return cls([_parse_entry(e, i) for i, e in enumerate(data)])

    @classmethod
    def load(cls, path: str | Path | None = None) -> Registry:
        p = Path(path) if path else DEFAULT_REGISTRY
        return cls.from_yaml(p.read_text(encoding="utf-8"))
