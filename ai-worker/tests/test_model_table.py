"""docs/07 section 5 is generated from registry.yaml (ai-worker/scripts/gen_model_table.py)."""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

import pytest

from imagepicker_ai.models import Registry

SCRIPT = Path(__file__).resolve().parents[1] / "scripts" / "gen_model_table.py"


@pytest.fixture(scope="module")
def gen():
    spec = importlib.util.spec_from_file_location("gen_model_table", SCRIPT)
    mod = importlib.util.module_from_spec(spec)
    sys.modules["gen_model_table"] = mod
    spec.loader.exec_module(mod)
    return mod


def test_docs_07_matches_the_registry(gen):
    if not gen.DOC.is_file():
        pytest.skip("docs/ not present (worker packaged on its own)")
    cur = gen.DOC.read_text(encoding="utf-8")
    assert gen.splice(cur, gen.render_table(Registry.load())) == cur, (
        "docs/07 section 5 is out of date; run: cd ai-worker && uv run python scripts/gen_model_table.py"
    )
    assert gen.main(["--check"]) == 0


def test_table_has_one_row_per_model_and_flags_open_questions(gen):
    reg = Registry.load()
    table = gen.render_table(reg).splitlines()
    assert len(table) == 2 + len(reg)
    row = next(r for r in table if r.startswith("| `nima-aesthetic` |"))
    assert "`research-only`" in row and "⚠️ `unknown`" in row and "AVA" in row
    ok = next(r for r in table if r.startswith("| `yunet` |"))
    assert "⚠️" not in ok and "`commercial-ok`" in ok


def test_stale_doc_is_detected_and_fixed(gen, tmp_path):
    doc = tmp_path / "07.md"
    doc.write_text(f"intro\n{gen.BEGIN}\n\nold table\n\n{gen.END}\noutro\n", encoding="utf-8")
    assert gen.main(["--doc", str(doc), "--check"]) == 1
    assert gen.main(["--doc", str(doc)]) == 0
    assert gen.main(["--doc", str(doc), "--check"]) == 0
    text = doc.read_text(encoding="utf-8")
    assert text.startswith("intro\n") and text.endswith("outro\n") and "old table" not in text
    assert "`skyseg-u2net`" in text


def test_missing_markers_are_an_error(gen):
    with pytest.raises(SystemExit):
        gen.splice("no markers here", "t")
