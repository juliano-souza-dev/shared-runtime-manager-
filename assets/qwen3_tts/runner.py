"""Persistent JSON-lines worker for the Shared Qwen3-TTS CPU runtime."""
import contextlib
import json
import os
import sys
import traceback
from pathlib import Path

import soundfile as sf
import torch
from qwen_tts import Qwen3TTSModel


def main() -> int:
    if len(sys.argv) != 2:
        print("Usage: runner.py <model-directory>", file=sys.stderr)
        return 2

    model_path = Path(sys.argv[1]).resolve()
    if not (model_path / "config.json").is_file():
        print(f"Qwen3-TTS model not found: {model_path}", file=sys.stderr)
        return 2

    # Explicit CPU execution: no CUDA, FlashAttention, or GPU runtime is required.
    cpu_count = os.cpu_count() or 4
    torch.set_num_threads(max(1, min(cpu_count, int(os.environ.get("SRM_TTS_THREADS", cpu_count)))))
    torch.set_num_interop_threads(1)

    with contextlib.redirect_stdout(sys.stderr):
        model = Qwen3TTSModel.from_pretrained(
            str(model_path),
            device_map="cpu",
            dtype=torch.float32,
            attn_implementation="sdpa",
        )

    for raw in sys.stdin:
        request = {}
        try:
            request = json.loads(raw)
            text = str(request.get("text", "")).strip()
            output_value = str(request.get("output_path", "")).strip()
            if not text:
                raise ValueError("The text field cannot be empty.")
            if not output_value:
                raise ValueError("The output_path field is required.")

            language = str(request.get("language", "English"))
            speaker = str(request.get("speaker", "Aiden"))
            output_path = Path(output_value).resolve()
            output_path.parent.mkdir(parents=True, exist_ok=True)

            with contextlib.redirect_stdout(sys.stderr):
                waveforms, sample_rate = model.generate_custom_voice(
                    text=text,
                    language=language,
                    speaker=speaker,
                )
            sf.write(str(output_path), waveforms[0], sample_rate)
            response = {
                "id": request.get("id"),
                "status": "completed",
                "output_path": str(output_path),
                "sample_rate": int(sample_rate),
                "speaker": speaker,
                "language": language,
            }
        except Exception as error:
            response = {
                "id": request.get("id"),
                "status": "error",
                "error": str(error),
                "traceback": traceback.format_exc(),
            }
        sys.stdout.write(json.dumps(response, ensure_ascii=False) + "\n")
        sys.stdout.flush()

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
