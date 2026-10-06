"""Export SigLIP2 (image + text towers) to ONNX when no ready-made export is available.

The default registry uses the pre-exported `onnx-community/siglip2-base-patch16-224-ONNX`, so this
script is only needed for other checkpoints (e.g. `google/siglip2-so400m-patch14-384`) or to
re-host exports under the project's own HF org.

  uv run --extra torch python scripts/export_siglip2_onnx.py \
      --model google/siglip2-base-patch16-224 --out .models/export/siglip2-base [--fp16]

Produces <out>/vision_model.onnx (pixel_values -> pooler_output) and <out>/text_model.onnx
(input_ids -> pooler_output) plus tokenizer/preprocessor files, with dynamic batch size.
Output names/shape match what imagepicker_ai.steps.embed expects.
"""

from __future__ import annotations

import argparse
from pathlib import Path


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--model", default="google/siglip2-base-patch16-224")
    ap.add_argument("--out", required=True)
    ap.add_argument("--opset", type=int, default=17)
    ap.add_argument("--fp16", action="store_true", help="also write *_fp16.onnx (needs `onnx`)")
    ap.add_argument("--text-len", type=int, default=64)
    ap.add_argument("--skip-text", action="store_true")
    args = ap.parse_args()

    import torch
    from transformers import AutoModel, AutoProcessor

    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    model = AutoModel.from_pretrained(args.model, dtype=torch.float32).eval()
    proc = AutoProcessor.from_pretrained(args.model)
    proc.save_pretrained(out)

    class Vision(torch.nn.Module):
        def __init__(self, m):
            super().__init__()
            self.m = m.vision_model

        def forward(self, pixel_values):
            o = self.m(pixel_values=pixel_values)
            return o.last_hidden_state, o.pooler_output

    class Text(torch.nn.Module):
        def __init__(self, m):
            super().__init__()
            self.m = m.text_model

        def forward(self, input_ids):
            o = self.m(input_ids=input_ids)
            return o.last_hidden_state, o.pooler_output

    size = model.config.vision_config.image_size
    jobs = [
        (
            "vision_model.onnx",
            Vision(model),
            (torch.randn(2, 3, size, size),),
            ["pixel_values"],
            {"pixel_values": {0: "batch_size"}},
        )
    ]
    if not args.skip_text:
        ids = torch.randint(0, 1000, (2, args.text_len), dtype=torch.long)
        jobs.append(
            (
                "text_model.onnx",
                Text(model),
                (ids,),
                ["input_ids"],
                {"input_ids": {0: "batch_size"}},
            )
        )

    for name, mod, example, in_names, dyn in jobs:
        path = out / name
        dyn = {**dyn, "last_hidden_state": {0: "batch_size"}, "pooler_output": {0: "batch_size"}}
        with torch.inference_mode():
            torch.onnx.export(
                mod,
                example,
                str(path),
                input_names=in_names,
                output_names=["last_hidden_state", "pooler_output"],
                dynamic_axes=dyn,
                opset_version=args.opset,
                dynamo=False,
            )
        print("wrote", path, f"{path.stat().st_size / 1e6:.0f} MB")
        if args.fp16:
            import onnx
            from onnxruntime.transformers.float16 import convert_float_to_float16

            m16 = convert_float_to_float16(onnx.load(str(path)), keep_io_types=True)
            p16 = path.with_name(path.stem + "_fp16.onnx")
            onnx.save(m16, str(p16))
            print("wrote", p16)

    # sanity check against torch
    import numpy as np
    import onnxruntime as ort

    s = ort.InferenceSession(str(out / "vision_model.onnx"), providers=["CPUExecutionProvider"])
    x = torch.randn(1, 3, size, size)
    with torch.inference_mode():
        ref = model.vision_model(pixel_values=x).pooler_output.numpy()
    got = s.run(["pooler_output"], {"pixel_values": x.numpy()})[0]
    cos = float((ref[0] @ got[0]) / (np.linalg.norm(ref[0]) * np.linalg.norm(got[0])))
    print(f"vision parity cosine vs torch: {cos:.6f}")
    assert cos > 0.999


if __name__ == "__main__":
    main()
