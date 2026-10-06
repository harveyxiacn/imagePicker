STEPS = ("phash", "quality", "faces", "embed")
# step names the Core may send (doc 05 §4) that map onto ours or are not implemented yet
STEP_ALIASES = {
    "iqa": "quality",
    "sharpness": "quality",
    "exposure": "quality",
    "embedding": "embed",
}
