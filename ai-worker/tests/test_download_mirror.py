"""A chosen download mirror is the very first host contacted (no request to the default host first).

The HTTP layer (`huggingface_hub.hf_hub_download`) is replaced by a recorder; nothing touches the
network.
"""

from __future__ import annotations

import hashlib

import pytest

from imagepicker_ai.errors import DownloadFailed
from imagepicker_ai.models import Downloader, Registry

OFFICIAL = "https://huggingface.co"
MIRROR = "https://hf-mirror.com"
PAYLOAD = b"model-bytes"


@pytest.fixture
def spec():
    sha = hashlib.sha256(PAYLOAD).hexdigest()
    reg = Registry.from_yaml(
        f"- {{id: m, task: [t], backend: onnx, source: {{hf: org/repo}}, "
        f"files: [{{path: m.onnx, size: {len(PAYLOAD)}, sha256: {sha}}}]}}"
    )
    return reg.get("m")


@pytest.fixture
def fake_http(monkeypatch):
    """Replace hf_hub_download; returns the list of (endpoint, filename) in request order."""
    import huggingface_hub

    calls: list[tuple[str, str]] = []
    down: set[str] = set()

    def fake(*, repo_id, filename, local_dir, endpoint, tqdm_class=None, **_kw):
        calls.append((endpoint, filename))
        if endpoint in down:
            raise ConnectionError(f"{endpoint} unreachable")
        out = local_dir / filename
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_bytes(PAYLOAD)
        return str(out)

    monkeypatch.setattr(huggingface_hub, "hf_hub_download", fake)
    calls_down = (calls, down)
    return calls_down


def _env(monkeypatch, endpoint=None, source=None):
    for k, v in (("HF_ENDPOINT", endpoint), ("IMAGEPICKER_MODEL_SOURCE", source)):
        if v is None:
            monkeypatch.delenv(k, raising=False)
        else:
            monkeypatch.setenv(k, v)


def test_mirror_choice_first_request_goes_to_mirror(tmp_path, monkeypatch, fake_http, spec):
    calls, _down = fake_http
    _env(monkeypatch, endpoint=MIRROR, source="hf-mirror")  # what the app sets for "HF mirror"
    Downloader(tmp_path).ensure(spec)  # default endpoint list, as the worker builds it
    assert calls == [(MIRROR, "m.onnx")]  # the default host was never contacted


def test_source_only_env_also_prefers_the_mirror(tmp_path, monkeypatch, fake_http, spec):
    calls, _down = fake_http
    _env(monkeypatch, source="hf-mirror")
    Downloader(tmp_path).ensure(spec)
    assert calls[0][0] == MIRROR and len(calls) == 1


def test_default_source_still_starts_at_official(tmp_path, monkeypatch, fake_http, spec):
    calls, _down = fake_http
    _env(monkeypatch)
    Downloader(tmp_path).ensure(spec)
    assert calls == [(OFFICIAL, "m.onnx")]


def test_unreachable_mirror_falls_back_to_official(tmp_path, monkeypatch, fake_http, spec):
    calls, down = fake_http
    _env(monkeypatch, endpoint="https://my.mirror")
    down.add("https://my.mirror")
    Downloader(tmp_path).ensure(spec)
    assert [c[0] for c in calls] == ["https://my.mirror", OFFICIAL]


def test_every_endpoint_down_reports_all_errors(tmp_path, monkeypatch, fake_http, spec):
    calls, down = fake_http
    _env(monkeypatch, endpoint=MIRROR)
    down.update({OFFICIAL, MIRROR})
    with pytest.raises(DownloadFailed) as ei:
        Downloader(tmp_path).ensure(spec)
    assert [c[0] for c in calls] == [MIRROR, OFFICIAL]
    assert len(ei.value.extra["errors"]) == 2
