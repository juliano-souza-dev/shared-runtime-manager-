"""Persistent JSON-lines worker for Qwen3-TTS Base and VoiceDesign profiles."""
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
                # Speaker embedding only: avoid carrying room acoustics,
                # reverberation, and recording coloration into generated cards.
                x_vector_only_mode=True,
            )
        set_stage("voice_prompt_saving")
        torch.save(items, profile_path / "voice_clone_prompt.pt")
        (profile_path / "voice_clone_mode.txt").write_text("speaker_only", encoding="utf-8")
        return {
            "status": "completed",
            "profile_path": str(profile_path),
            "voice_clone_mode": "speaker_only",
        }

    if operation == "prepare_voice_design":
        instruct = str(request.get("instruct", "")).strip()
        if not 10 <= len(instruct) <= 800:
            raise ValueError("A descrição da voz precisa ter entre 10 e 800 caracteres.")
        set_stage("voice_design_profile_saving")
        (profile_path / "voice_design_instruct.txt").write_text(instruct, encoding="utf-8")
        return {"status": "completed", "profile_path": str(profile_path), "voice_mode": "voice_design"}

    if operation == "synthesize":
        voice_mode = str(request.get("voice_mode", "voice_clone"))
        if voice_mode == "voice_design":
            instruct = str(request.get("instruct") or (profile_path / "voice_design_instruct.txt").read_text(encoding="utf-8")).strip()
            if not 10 <= len(instruct) <= 800:
                raise ValueError("A descrição da voz precisa ter entre 10 e 800 caracteres.")
        elif voice_mode != "voice_clone":
            raise ValueError("Modo de voz Qwen desconhecido.")
        prompt_path = profile_path / "voice_clone_prompt.pt"
        set_stage("voice_prompt_loading")
        prompt = torch.load(prompt_path, map_location="cpu", weights_only=False) if voice_mode == "voice_clone" else None
        # Rebuild legacy profiles once using speaker-only conditioning. This
        # prevents reference-room reverberation from being baked into future
        # generations while preserving the existing profile and reference WAV.
        mode_path = profile_path / "voice_clone_mode.txt"
        current_mode = mode_path.read_text(encoding="utf-8").strip() if mode_path.is_file() else ""
        if voice_mode == "voice_clone" and current_mode != "speaker_only":
            reference_audio = request.get("reference_audio") or str(profile_path / "reference.wav")
            reference_text = str(request.get("reference_text") or "").strip()
            # Some existing app integrations send only the profile directory.
            # Keep their legacy prompt usable; newly created profiles already
            # use speaker-only conditioning from prepare_voice.
            if Path(reference_audio).is_file() and reference_text:
                set_stage("reference_audio_encoding")
                with contextlib.redirect_stdout(sys.stderr):
                    prompt = model.create_voice_clone_prompt(
                        ref_audio=str(Path(reference_audio).resolve()),
                        ref_text=reference_text,
                        x_vector_only_mode=True,
                    )
                set_stage("voice_prompt_saving")
                torch.save(prompt, prompt_path)
                mode_path.write_text("speaker_only", encoding="utf-8")
        if voice_mode == "voice_clone" and (not isinstance(prompt, (list, tuple)) or not prompt):
            raise ValueError("O perfil de voz contém um prompt Qwen inválido ou vazio.")
        text = str(request["text"]).strip()
        language = str(request.get("language", "English")).strip() or "English"
        # A terminal punctuation mark gives the model a clearer prosodic boundary
        # when an Anki expression arrives as a bare phrase.
        if text and text[-1] not in ".!?…;:":
            text += "."
        set_stage("voice_generation")
        with contextlib.redirect_stdout(sys.stderr):
            if voice_mode == "voice_design":
                segment_instruct = instruct
                if language.lower() == "english":
                    suffix = "Keep the same speaker identity and vocal timbre; use clear, natural American English pronunciation."
                    if suffix.lower() not in segment_instruct.lower() and len(segment_instruct) + len(suffix) + 2 <= 800:
                        segment_instruct = segment_instruct.rstrip(". ") + ". " + suffix
                wavs, sample_rate = model.generate_voice_design(
                    text=text, language=language, instruct=segment_instruct, do_sample=True
                )
            else:
                wavs, sample_rate = model.generate_voice_clone(
                    text=text, language=language, voice_clone_prompt=prompt,
                    non_streaming_mode=True, do_sample=True, top_k=50, top_p=0.95,
                    temperature=0.85, repetition_penalty=1.08,
                    subtalker_dosample=True, subtalker_top_k=50,
                    subtalker_top_p=0.95, subtalker_temperature=0.85,
                )
        set_stage("audio_pacing")
        # Qwen3-TTS Base does not expose a native speech-rate argument. Use a
        # pitch-preserving phase vocoder for a modest, configurable slowdown.
        import librosa
        import numpy as np
        speech_rate = float(os.environ.get("SRM_TTS_SPEECH_RATE", "0.92"))
        speech_rate = min(1.0, max(0.85, speech_rate))
        waveform = np.asarray(wavs[0], dtype=np.float32)
        if abs(speech_rate - 1.0) > 1e-3:
            waveform = librosa.effects.time_stretch(y=waveform, rate=speech_rate)
        # A short tail avoids the abrupt cutoff common in isolated flashcards.
        tail = np.zeros(int(sample_rate * 0.14), dtype=waveform.dtype)
        waveform = np.concatenate((waveform, tail))
        set_stage("audio_file_writing")
        output = Path(request["output_path"]).resolve()
        output.parent.mkdir(parents=True, exist_ok=True)
        sf.write(str(output), waveform, sample_rate)
        return {
            "status": "completed",
            "output_path": str(output),
            "sample_rate": int(sample_rate),
            "language": language,
            "speech_rate": speech_rate,
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
            try:
                model = Qwen3TTSModel.from_pretrained(
                    str(model_path), device_map="cpu", dtype=torch.float32,
                    low_cpu_mem_usage=True,
                )
                print("voice_loader=optimized", file=sys.stderr, flush=True)
            except Exception as optimized_error:
                print(f"voice_loader=legacy_fallback reason={type(optimized_error).__name__}: {optimized_error}", file=sys.stderr, flush=True)
                model = Qwen3TTSModel.from_pretrained(
                    str(model_path), device_map={"": "cpu"}, dtype=torch.float32,
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
