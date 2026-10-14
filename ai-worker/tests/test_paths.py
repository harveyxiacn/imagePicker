"""The worker takes its folders from what the app hands it (portable mode keeps them next to the
executable); it never falls back to a hardcoded profile path while the environment says better."""

from pathlib import Path

from imagepicker_ai.paths import default_data_dir, resolve_models_dir


def _clean(monkeypatch):
    monkeypatch.delenv("IMAGEPICKER_MODELS_DIR", raising=False)
    monkeypatch.delenv("IMAGEPICKER_DATA_DIR", raising=False)


def test_cli_value_beats_env(tmp_path, monkeypatch):
    _clean(monkeypatch)
    monkeypatch.setenv("IMAGEPICKER_MODELS_DIR", str(tmp_path / "env"))
    monkeypatch.setenv("IMAGEPICKER_DATA_DIR", str(tmp_path / "data"))
    assert resolve_models_dir(tmp_path / "cli") == (tmp_path / "cli").resolve()


def test_models_env_beats_data_env(tmp_path, monkeypatch):
    _clean(monkeypatch)
    monkeypatch.setenv("IMAGEPICKER_MODELS_DIR", str(tmp_path / "env"))
    monkeypatch.setenv("IMAGEPICKER_DATA_DIR", str(tmp_path / "data"))
    assert resolve_models_dir() == (tmp_path / "env").resolve()


def test_portable_data_dir_gives_models_inside_it(tmp_path, monkeypatch):
    _clean(monkeypatch)
    monkeypatch.setenv("IMAGEPICKER_DATA_DIR", str(tmp_path / "app" / "data"))
    assert default_data_dir() == tmp_path / "app" / "data"
    got = resolve_models_dir()
    assert got == (tmp_path / "app" / "data" / "models").resolve()
    assert got.is_dir()


def test_profile_default_only_without_env(monkeypatch):
    _clean(monkeypatch)
    assert isinstance(default_data_dir(), Path)
    assert default_data_dir().name.lower() == "imagepicker"
