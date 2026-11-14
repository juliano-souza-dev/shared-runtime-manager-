"""One-shot Qwen3-TTS Base worker for preparing and using reusable voice profiles."""
import json
import sys
import traceback
from pathlib import Path

import soundfile as sf
import torch
from qwen_tts import Qwen3TTSModel


def main() -> int:
    if len(sys.argv) != 4:
        print("Usage: voice_runner.py <model-directory> <profile-directory>", file=sys.stderr)
        return 2
    model_path, profile_path = Path(sys.argv[1]).resolve(), Path(sys.argv[2]).resolve()
    try:
        request = json.loads(sys.stdin.readline())
        torch.set_num_threads(max(1, min(os.cpu_count() or 4, 8)))
        model = Qwen3TTSModel.from_pretrained(
            str(model_path), device_map={"": "cpu"}, dtype=torch.float32,
            attn_implementation="sdpa",
        )
        operation = request.get("operation")
        if operation == "prepare_voice":
            items = model.create_voice_clone_prompt(
                ref_audio=str(Path(request["reference_audio"]).resolve()),
                ref_text=str(request["reference_text"]),
                x_vector_only_mode=False,
            )
            prompt = [{
                "ref_code": item.ref_code,
                "ref_spk_embedding": item.ref_spk_embedding,
                "x_vector_only_mode": item.x_vector_only_mode,
                "icl_mode": item.icl_mode,
            } for item in items]
            torch.save(prompt, profile_path / "voice_clone_prompt.pt")
            result = {"status": "completed", "profile_path": str(profile_path)}
        elif operation == "synthesize":
            prompt_path = profile_path / "voice_clone_prompt.pt"
            prompt = torch.load(prompt_path, map_location="cpu", weights_only=False)
            wavs, sample_rate = model.generate_voice_clone(
                text=str(request["text"]), language=str(request.get("language", "Portuguese")),
                voice_clone_prompt=prompt,
            )
            output = Path(request["output_path"]).resolve()
            output.parent.mkdir(parents=True, exist_ok=True)
            sf.write(str(output), wavs[0], sample_rate)
            result = {"status": "completed", "output_path": str(output), "sample_rate": int(sample_rate)}
        else:
            raise ValueError("Unsupported operation: " + str(operation))
        print(json.dumps(result, ensure_ascii=False))
        return 0
    except Exception as error:
        print(json.dumps({"status": "error", "error": str(error), "traceback": traceback.format_exc()}, ensure_ascii=False))
        return 1


if __name__ == "__main__":
    import os
    raise SystemExit(main())
