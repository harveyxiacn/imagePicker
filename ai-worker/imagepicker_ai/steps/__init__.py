STEPS = (
    "phash",
    "quality",
    "faces",
    "identity",
    "embed",
    "aesthetic",
    "iqa",
    "scene",
)

# analyze.batch `profile` -> step list (docs/api-contract-m2.md section A)
PROFILES: dict[str, tuple[str, ...]] = {
    "fast": ("phash", "quality", "faces"),
    "standard": ("phash", "quality", "faces", "identity", "embed", "aesthetic", "iqa", "scene"),
}
DEFAULT_PROFILE = "standard"

# step names the Core may send (doc 05 section 4) that map onto ours or are not implemented yet.
# NB: `iqa` is its own step since M2 (it used to alias `quality`).
STEP_ALIASES = {
    "sharpness": "quality",
    "exposure": "quality",
    "embedding": "embed",
}
