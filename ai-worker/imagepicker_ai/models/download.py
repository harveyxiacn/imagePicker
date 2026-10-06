"""Model downloader: HF official -> $HF_ENDPOINT -> hf-mirror, resume, sha256, atomic move (doc 07 §1).

Layout:
  <models_dir>/<model id>/<file path>      final, verified files (+ .manifest.json = "complete" marker)
  <models_dir>/.partial/<model id>/...     in-flight downloads (resumable; removed on success)
"""

from __future__ import annotations

import hashlib
import json
import logging
import os
import shutil
import threading
import time
import urllib.error
import urllib.request
from collections.abc import Callable
from pathlib import Path
from typing import Any

from ..errors import DownloadFailed
from .registry import ModelFile, ModelSpec

log = logging.getLogger(__name__)

OFFICIAL_ENDPOINT = "https://huggingface.co"
MIRROR_ENDPOINT = "https://hf-mirror.com"
MANIFEST = ".manifest.json"

# progress event: {"model", "file", "bytes", "total", "phase"} ; phase = download | verify | done
ProgressCb = Callable[[dict[str, Any]], None]


class Cancelled(Exception):
    pass


def hf_endpoints() -> list[str]:
    """official -> HF_ENDPOINT (if set) -> hf-mirror.com, de-duplicated."""
    out: list[str] = []
    for ep in (OFFICIAL_ENDPOINT, os.environ.get("HF_ENDPOINT"), MIRROR_ENDPOINT):
        if ep:
            ep = ep.rstrip("/")
            if ep not in out:
                out.append(ep)
    return out


def sha256_file(path: Path, chunk: int = 1 << 20) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        while b := f.read(chunk):
            h.update(b)
    return h.hexdigest()


class Downloader:
    def __init__(self, models_dir: Path, endpoints: list[str] | None = None):
        self.models_dir = Path(models_dir)
        self.endpoints = endpoints if endpoints is not None else hf_endpoints()

    # ------------------------------------------------------------------ queries
    def install_dir(self, spec: ModelSpec) -> Path:
        return self.models_dir / spec.id

    def file_path(self, spec: ModelSpec, f: ModelFile | None = None) -> Path:
        return self.install_dir(spec) / (f or spec.primary).path

    def is_installed(self, spec: ModelSpec) -> bool:
        d = self.install_dir(spec)
        if not (d / MANIFEST).is_file():
            return False
        for f in spec.files:
            p = d / f.path
            if not p.is_file():
                return False
            if f.size is not None and p.stat().st_size != f.size:
                return False
        return True

    def installed_size(self, spec: ModelSpec) -> int:
        d = self.install_dir(spec)
        return sum(p.stat().st_size for p in d.rglob("*") if p.is_file()) if d.exists() else 0

    def remove(self, spec: ModelSpec) -> None:
        shutil.rmtree(self.install_dir(spec), ignore_errors=True)
        shutil.rmtree(self.models_dir / ".partial" / spec.id, ignore_errors=True)

    # ------------------------------------------------------------------ download
    def ensure(
        self,
        spec: ModelSpec,
        progress: ProgressCb | None = None,
        cancel: threading.Event | None = None,
    ) -> Path:
        """Make `spec` available locally (downloading when needed). Returns the primary file path."""
        cancel = cancel or threading.Event()
        emit = progress or (lambda _e: None)
        if self.is_installed(spec):
            return self.file_path(spec)

        dest = self.install_dir(spec)
        partial = self.models_dir / ".partial" / spec.id
        partial.mkdir(parents=True, exist_ok=True)
        dest.mkdir(parents=True, exist_ok=True)
        # progress is aggregated over the whole model
        total_known = sum(f.size or 0 for f in spec.files)
        done_before = 0
        used_endpoint = None
        try:
            for f in spec.files:
                final = dest / f.path
                if self._file_ok(final, f):
                    done_before += f.size or final.stat().st_size
                    continue
                final.parent.mkdir(parents=True, exist_ok=True)

                peak = [0]

                def on_bytes(
                    n: int,
                    total: int | None,
                    _f: ModelFile = f,
                    _base: int = done_before,
                    _peak: list[int] = peak,
                ) -> None:
                    if cancel.is_set():
                        raise Cancelled()
                    _peak[0] = max(_peak[0], n)  # backends may report out-of-order chunk counts
                    done = _base + _peak[0]
                    tot = total_known or ((_base + total) if total else None)
                    emit(
                        {
                            "model": spec.id,
                            "file": _f.path,
                            "phase": "download",
                            "bytes": done,
                            "total": max(tot, done) if tot else None,
                        }
                    )

                got, used_endpoint = self._fetch_file(spec, f, partial, on_bytes, cancel)
                emit(
                    {
                        "model": spec.id,
                        "file": f.path,
                        "phase": "verify",
                        "bytes": done_before,
                        "total": total_known or None,
                    }
                )
                self._verify(got, f)
                os.replace(got, final)  # atomic on the same volume
                done_before += final.stat().st_size
            manifest = {
                "id": spec.id,
                "files": {f.path: (dest / f.path).stat().st_size for f in spec.files},
                "endpoint": used_endpoint,
                "completed_at": int(time.time()),
            }
            tmp = dest / (MANIFEST + ".tmp")
            tmp.write_text(json.dumps(manifest), encoding="utf-8")
            os.replace(tmp, dest / MANIFEST)
        except Cancelled:
            raise
        shutil.rmtree(partial, ignore_errors=True)
        emit(
            {
                "model": spec.id,
                "file": None,
                "phase": "done",
                "bytes": done_before,
                "total": done_before,
            }
        )
        return self.file_path(spec)

    # ------------------------------------------------------------------ internals
    @staticmethod
    def _file_ok(p: Path, f: ModelFile) -> bool:
        if not p.is_file():
            return False
        if f.size is not None and p.stat().st_size != f.size:
            return False
        if f.sha256 is not None:
            return sha256_file(p) == f.sha256
        return True

    @staticmethod
    def _verify(p: Path, f: ModelFile) -> None:
        if f.size is not None and p.stat().st_size != f.size:
            raise DownloadFailed(f"size mismatch for {f.path}: {p.stat().st_size} != {f.size}")
        if f.sha256 is not None:
            actual = sha256_file(p)
            if actual != f.sha256:
                p.unlink(missing_ok=True)
                raise DownloadFailed(f"sha256 mismatch for {f.path}: {actual} != {f.sha256}")

    def _fetch_file(
        self,
        spec: ModelSpec,
        f: ModelFile,
        partial: Path,
        on_bytes: Callable[[int, int | None], None],
        cancel: threading.Event,
    ) -> tuple[Path, str]:
        errors: list[str] = []
        if f.url:  # plain HTTP source
            try:
                return self._http_get(f.url, partial / f.path, f, on_bytes), f.url
            except Cancelled:
                raise
            except Exception as e:  # noqa: BLE001
                raise DownloadFailed(f"{f.path}: {e}", {"errors": [str(e)]}) from e

        for ep in self.endpoints:
            if cancel.is_set():
                raise Cancelled()
            try:
                p = self._hf_get(spec, f, partial, ep, on_bytes)
                self._verify(p, f)
                return p, ep
            except Cancelled:
                raise
            except Exception as e:  # noqa: BLE001
                log.warning("download of %s/%s via %s failed: %s", spec.id, f.path, ep, e)
                errors.append(f"{ep}: {e}")
        raise DownloadFailed(
            f"could not download {spec.id}/{f.path} from any endpoint", {"errors": errors}
        )

    def _hf_get(
        self,
        spec: ModelSpec,
        f: ModelFile,
        partial: Path,
        endpoint: str,
        on_bytes: Callable[[int, int | None], None],
    ) -> Path:
        from huggingface_hub import hf_hub_download
        from tqdm.auto import tqdm

        class _Tqdm(tqdm):
            def __init__(self, *a: Any, **kw: Any):
                kw["disable"] = True  # no console noise; stdout is reserved for the ready line
                super().__init__(*a, **kw)
                self._n = int(kw.get("initial") or 0)
                self._total = kw.get("total")

            def update(self, n: float | None = 1) -> bool | None:
                self._n += int(n or 0)
                on_bytes(self._n, self._total)
                return super().update(n)

        path = hf_hub_download(
            repo_id=spec.hf_repo,  # type: ignore[arg-type]
            filename=f.path,
            revision=spec.hf_revision,
            local_dir=partial,
            endpoint=endpoint,
            etag_timeout=10,
            tqdm_class=_Tqdm,
        )
        return Path(path)

    @staticmethod
    def _http_get(
        url: str, target: Path, f: ModelFile, on_bytes: Callable[[int, int | None], None]
    ) -> Path:
        """Resumable (Range) download to `target`."""
        target.parent.mkdir(parents=True, exist_ok=True)
        part = target.with_name(target.name + ".part")
        for _attempt in range(2):
            have = part.stat().st_size if part.exists() else 0
            req = urllib.request.Request(url, headers={"User-Agent": "imagepicker-ai/0.1"})
            if have:
                req.add_header("Range", f"bytes={have}-")
            try:
                resp = urllib.request.urlopen(req, timeout=30)  # noqa: S310
            except urllib.error.HTTPError as e:
                if e.code == 416:  # range not satisfiable: partial is complete or stale
                    part.unlink(missing_ok=True)
                    continue
                raise
            with resp:
                if have and resp.status != 206:
                    have = 0  # server ignored Range
                length = resp.headers.get("Content-Length")
                total = (int(length) + have) if length else None
                with open(part, "ab" if have else "wb") as out:
                    n = have
                    while chunk := resp.read(1 << 20):
                        out.write(chunk)
                        n += len(chunk)
                        on_bytes(n, total)
            os.replace(part, target)
            return target
        raise DownloadFailed(f"could not download {url}")
