"""Generate the model / licence table of docs/07 section 5 from `registry.yaml`.

    uv run python scripts/gen_model_table.py            # rewrite the generated block in docs/07
    uv run python scripts/gen_model_table.py --check    # exit 1 when docs/07 is out of date

The block sits between the BEGIN / END markers below; everything outside it is hand-written.
`tests/test_model_table.py` runs the check, so a registry change without a regenerated doc fails CI.
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path
from urllib.parse import urlparse

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))

from imagepicker_ai.models.registry import ModelSpec, Registry  # noqa: E402

DOC = HERE.parents[1] / "docs" / "07-模型清单与许可.md"
BEGIN = (
    "<!-- BEGIN GENERATED: models (ai-worker/scripts/gen_model_table.py; do not edit by hand) -->"
)
END = "<!-- END GENERATED: models -->"

HEADER = (
    "| 注册表 id | 任务 | 来源 | 体积 | 权重许可 | 训练数据 | 训练数据条款 | 商用标记 | 备注 |\n"
    "|---|---|---|---|---|---|---|---|---|"
)


def _cell(text: str | None) -> str:
    return (text or "—").replace("|", "\\|").replace("\n", " ").strip() or "—"


def _size(mb: float) -> str:
    return f"{mb / 1024:.1f}GB" if mb >= 1024 else f"{mb:g}MB"


def _source(s: ModelSpec) -> str:
    if s.hf_repo:
        return f"HF `{s.hf_repo}`"
    host = next((urlparse(f.url).netloc for f in s.files if f.url), "")
    return f"{host} (`{s.origin}`)" if host and s.origin else _cell(host)


def _usage(s: ModelSpec) -> str:
    flag = f"`{s.usage}`"
    return flag if s.usage == "commercial-ok" else f"⚠️ {flag}"


def render_table(reg: Registry) -> str:
    rows = [HEADER]
    for s in reg:
        rows.append(
            "| "
            + " | ".join(
                [
                    f"`{s.id}`",
                    _cell(", ".join(s.task)),
                    _cell(_source(s)),
                    _size(s.size_mb),
                    _cell(s.license),
                    _cell(s.training_data),
                    f"`{s.training_data_terms}`",
                    _usage(s),
                    _cell(s.license_note),
                ]
            )
            + " |"
        )
    return "\n".join(rows)


def splice(doc: str, table: str) -> str:
    """`doc` with the text between the markers replaced by `table`."""
    a = doc.find(BEGIN)
    b = doc.find(END)
    if a < 0 or b < a:
        raise SystemExit(f"docs/07: generated-block markers missing ({BEGIN!r} ... {END!r})")
    return doc[: a + len(BEGIN)] + "\n\n" + table + "\n\n" + doc[b:]


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--check", action="store_true", help="fail if docs/07 is out of date")
    ap.add_argument("--doc", type=Path, default=DOC)
    args = ap.parse_args(argv)
    cur = args.doc.read_text(encoding="utf-8")
    new = splice(cur, render_table(Registry.load()))
    if new == cur:
        print("docs/07 model table is up to date")
        return 0
    if args.check:
        print(
            "docs/07 model table is out of date; run: uv run python scripts/gen_model_table.py",
            file=sys.stderr,
        )
        return 1
    args.doc.write_text(new, encoding="utf-8", newline="\n")
    print(f"updated {args.doc}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
