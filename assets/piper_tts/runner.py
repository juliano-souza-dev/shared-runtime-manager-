"""Persistent JSON-lines worker for the Shared Piper CPU runtime."""
import json
import sys
import traceback
import wave
from pathlib import Path

from piper import PiperVoice


def main() -> int:
    if len(sys.argv) != 2:
        print("Usage: runner.py <voice.onnx>", file=sys.stderr)
        return 2

    voice_path = Path(sys.argv[1]).resolve()
    if not voice_path.is_file():
        print(f"Piper voice not found: {voice_path}", file=sys.stderr)
        return 2

    # Piper uses ONNX Runtime's CPU provider. CUDA is deliberately not enabled.
    voice = PiperVoice.load(str(voice_path), use_cuda=False)

    for raw in sys.stdin:
        request = {}
        try:
            request = json.loads(raw)
            text = str(request.get("text", "")).strip()
            output_path = Path(str(request.get("output_path", ""))).resolve()
            if not text:
                raise ValueError("The text field cannot be empty.")
            if not str(request.get("output_path", "")).strip():
                raise ValueError("The output_path field is required.")

            output_path.parent.mkdir(parents=True, exist_ok=True)
            with wave.open(str(output_path), "wb") as output:
                voice.synthesize_wav(text, output)

            response = {
                "id": request.get("id"),
                "status": "completed",
                "output_path": str(output_path),
            }
        except Exception as error:
            response = {
                "id": request.get("id") if "request" in locals() else None,
                "status": "error",
                "error": str(error),
                "traceback": traceback.format_exc(),
            }
        sys.stdout.write(json.dumps(response, ensure_ascii=False) + "\n")
        sys.stdout.flush()

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
