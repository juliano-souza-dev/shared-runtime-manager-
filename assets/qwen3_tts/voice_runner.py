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


def handle_request(model, profile_path: Path, request: dict, set_stage) -> dict:
    operation = request.get("operation")
    if operation == "prepare_voice":
        set_stage("reference_audio_encoding")
        with contextlib.redirect_stdout(sys.stderr):
            items = model.create_voice_clone_prompt(
                ref_audio=str(Path(request["reference_audio"]).resolve()),
                ref_text=str(request["reference_text"]),
                x_vector_only_mode=False,
            )
        # Keep the complete official prompt objects. Qwen needs ref_text to
        # rebuild the reference token IDs during generate_voice_clone().
        set_stage("voice_prompt_saving")
        torch.save(items, profile_path / "voice_clone_prompt.pt")
        return {"status": "completed", "profile_path": str(profile_path)}

    if operation == "synthesize":
        prompt_path = profile_path / "voice_clone_prompt.pt"
        set_stage("voice_prompt_loading")
        prompt = torch.load(prompt_path, map_location="cpu", weights_only=False)
        # Older builds stored an incomplete dict (or list of dicts) that
        # omitted ref_text. Rebuild it from the profile's normalized reference
        # audio and the transcript retained in the voice registry.
        if isinstance(prompt, dict) or (
            isinstance(prompt, list) and prompt and isinstance(prompt[0], dict)
        ):
            reference_audio = request.get("reference_audio") or str(profile_path / "reference.wav")
            reference_text = str(request.get("reference_text") or "").strip()
            if not Path(reference_audio).is_file():
                raise FileNotFoundError(
                    "Este perfil usa um formato antigo e não possui o áudio de referência necessário para migrá-lo."
                )
            if not reference_text:
                raise ValueError(
                    "Este perfil usa um formato antigo. Recrie o perfil para salvar a transcrição de referência."
                )
            set_stage("reference_audio_encoding")
            with contextlib.redirect_stdout(sys.stderr):
                prompt = model.create_voice_clone_prompt(
                    ref_audio=str(Path(reference_audio).resolve()),
                    ref_text=reference_text,
                    x_vector_only_mode=False,
                )
            set_stage("voice_prompt_saving")
            torch.save(prompt, prompt_path)
        if not isinstance(prompt, (list, tuple)) or not prompt:
            raise ValueError("O perfil de voz contém um prompt Qwen inválido ou vazio.")
        set_stage("voice_generation")
        with contextlib.redirect_stdout(sys.stderr):
            wavs, sample_rate = model.generate_voice_clone(
                text=str(request["text"]),
                language=str(request.get("language", "Portuguese")),
                voice_clone_prompt=prompt,
            )
        set_stage("audio_file_writing")
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
    stage = "runtime_initialization"
    try:
        cpu_count = os.cpu_count() or 4
        torch.set_num_threads(max(1, min(cpu_count, 8)))
        torch.set_num_interop_threads(1)
        stage = "model_loading"
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
            "id": None,
            "operation": "startup",
            "stage": stage,
            "error_type": type(error).__name__,
            "error": str(error),
            "traceback": traceback.format_exc(),
        }, ensure_ascii=False), flush=True)
        return 1

    for line_number, raw in enumerate(sys.stdin, start=1):
        request = {}
        operation = "request_parsing"
        stage = "request_parsing"
        try:
            request = json.loads(raw)
            stage = "request_validation"
            if not isinstance(request, dict):
                raise TypeError("A solicitação recebida precisa ser um objeto JSON.")
            operation = str(request.get("operation") or "unknown")
            stage = "voice_prompt_loading" if operation == "synthesize" else "voice_prompt_preparation"
            def set_stage(value):
                nonlocal stage
                stage = value
            response = handle_request(model, profile_path, request, set_stage)
            response.update({
                "id": request.get("id"),
                "operation": operation,
                "stage": "completed",
            })
        except Exception as error:
            response = {
                "status": "error",
                "id": request.get("id") if isinstance(request, dict) else None,
                "operation": operation,
                "stage": stage,
                "line": line_number,
                "error_type": type(error).__name__,
                "error": str(error),
                "traceback": traceback.format_exc(),
            }
        print(json.dumps(response, ensure_ascii=False), flush=True)

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
