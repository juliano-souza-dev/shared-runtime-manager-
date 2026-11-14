"""Persistent JSON-lines worker for Qwen3-TTS Base voice profiles."""
import contextlib
import json
import os
import sys
import traceback
from pathlib import Path

import soundfile as sf
import torch
from qwen_tts import Qwen3TTSModel


def handle_request(model, profile_path: Path, request: dict) -> dict:
    operation = request.get("operation")
    if operation == "prepare_voice":
        with contextlib.redirect_stdout(sys.stderr):
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
        return {"status": "completed", "profile_path": str(profile_path)}

    if operation == "synthesize":
        prompt_path = profile_path / "voice_clone_prompt.pt"
        prompt = torch.load(prompt_path, map_location="cpu", weights_only=False)
        with contextlib.redirect_stdout(sys.stderr):
            wavs, sample_rate = model.generate_voice_clone(
                text=str(request["text"]),
                language=str(request.get("language", "Portuguese")),
                voice_clone_prompt=prompt,
            )
        output = Path(request["output_path"]).resolve()
        output.parent.mkdir(parents=True, exist_ok=True)
        sf.write(str(output), wavs[0], sample_rate)
        return {
            "status": "completed",
            "output_path": str(output),
            "sample_rate": int(sample_rate),
        }

    raise ValueError("Unsupported operation: " + str(operation))


def main() -> int:
    if len(sys.argv) != 3:
        print("Usage: voice_runner.py <model-directory> <profile-directory>", file=sys.stderr)
        return 2

    model_path = Path(sys.argv[1]).resolve()
    profile_path = Path(sys.argv[2]).resolve()
    try:
        cpu_count = os.cpu_count() or 4
        torch.set_num_threads(max(1, min(cpu_count, 8)))
        torch.set_num_interop_threads(1)
        with contextlib.redirect_stdout(sys.stderr):
            model = Qwen3TTSModel.from_pretrained(
                str(model_path),
                device_map={"": "cpu"},
                dtype=torch.float32,
                attn_implementation="sdpa",
            )
    except Exception as error:
        print(json.dumps({
            "status": "error",
            "error": str(error),
            "traceback": traceback.format_exc(),
        }, ensure_ascii=False), flush=True)
        return 1

    for raw in sys.stdin:
        try:
            request = json.loads(raw)
            response = handle_request(model, profile_path, request)
            response["id"] = request.get("id")
        except Exception as error:
            response = {
                "status": "error",
                "id": None,
                "error": str(error),
                "traceback": traceback.format_exc(),
            }
        print(json.dumps(response, ensure_ascii=False), flush=True)

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
