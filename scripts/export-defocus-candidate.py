"""Developer-only D-DFFNet export; PyTorch is NOT an application dependency.

Requires the author's checkout, locally obtained weights, torch/torchvision,
resnest (per upstream), onnx and onnxruntime. Export the ResNest D-DFFNet tested
by upstream test.py. No downloads or redistribution of third-party weights.
"""
import argparse
import hashlib
import json
from pathlib import Path
import sys


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", type=Path, required=True)
    parser.add_argument("--checkpoint", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--positive-class", choices=["defocus", "focus"], required=True,
                        help="Must be verified from checkpoint training labels, not guessed")
    args = parser.parse_args()
    import numpy as np
    import torch
    import onnx
    import onnxruntime as ort

    sys.path.insert(0, str(args.repo.resolve()))
    from model.ours.PDNet_resnest import PDnet_ResNest_4F_SL_LA

    model = PDnet_ResNest_4F_SL_LA()
    # No executable pickle loads. Checkpoint must be a plain state dictionary.
    model.load_state_dict(torch.load(args.checkpoint, map_location="cpu", weights_only=True))
    model.eval()

    class Wrapper(torch.nn.Module):
        def __init__(self):
            super().__init__()
            self.model = model

        def forward(self, pixels):
            logits, _, _ = self.model(pixels)
            mask = torch.sigmoid(logits)
            if args.positive_class == "focus":
                mask = 1 - mask
            return torch.nn.functional.interpolate(mask, size=(320, 320), mode="nearest")

    wrapper = Wrapper().eval()
    torch.manual_seed(0)
    example = torch.rand(1, 3, 320, 320)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    torch.onnx.export(wrapper, example, str(args.output), input_names=["pixels"],
                      output_names=["blur_map"], opset_version=17, dynamo=False)
    onnx.checker.check_model(onnx.load(args.output))
    session = ort.InferenceSession(str(args.output), providers=["CPUExecutionProvider"])
    with torch.no_grad():
        expected = wrapper(example).numpy()
    actual = session.run(["blur_map"], {"pixels": example.numpy()})[0]
    np.testing.assert_allclose(actual, expected, rtol=1e-3, atol=1e-4)
    if actual.shape != (1, 1, 320, 320) or not np.isfinite(actual).all():
        raise ValueError("Invalid export output")
    manifest = {"schemaVersion": 1, "modelVersion": args.version,
                "contract": "rgb-imagenet-320-sigmoid-blur-v1",
                "sha256": hashlib.sha256(args.output.read_bytes()).hexdigest()}
    args.output.with_suffix(".json").write_text(json.dumps(manifest, indent=2)+"\n", encoding="utf-8")
    print(f"Verified export: {args.output}")


if __name__ == "__main__":
    main()
