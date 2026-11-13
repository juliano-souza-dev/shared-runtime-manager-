#!/usr/bin/env python3
"""Persistent JSON-lines worker for word-level forced alignment.

The Shared Runtime Manager owns this script, its virtual environment and model cache.
One process can serve multiple transcript blocks so acoustic models stay loaded.
"""
import contextlib
import json
import os
import sys
import traceback
from pathlib import Path

_SHARED_ROOT = Path(__file__).resolve().parents[2]
_SHARED_MODELS = _SHARED_ROOT / "models" / "forced-alignment"
os.environ.setdefault("HF_HOME", str(_SHARED_MODELS / "huggingface"))
os.environ.setdefault("TORCH_HOME", str(_SHARED_MODELS / "torch"))
os.environ.setdefault("XDG_CACHE_HOME", str(_SHARED_ROOT / "cache" / "forced-alignment"))

os.environ.setdefault("TOKENIZERS_PARALLELISM", "false")
os.environ.setdefault("HF_HUB_DISABLE_TELEMETRY", "1")

_models = {}
_device = None


def emit(payload):
    sys.stdout.write(json.dumps(payload, ensure_ascii=False, separators=(",", ":")) + "\n")
    sys.stdout.flush()


def get_device():
    global _device
    if _device is None:
        import torch
        _device = "cuda" if torch.cuda.is_available() else "cpu"
    return _device


def get_alignment_model(language):
    if language not in _models:
        import whisperx
        device = get_device()
        with contextlib.redirect_stdout(sys.stderr):
            model, metadata = whisperx.load_align_model(
                language_code=language,
                device=device,
            )
        _models[language] = (model, metadata)
    return _models[language]


def align_block(request):
    import whisperx

    request_id = request.get("requestId")
    audio_path = request.get("audioPath")
    text = request.get("text")
    language = request.get("language")
    if not isinstance(audio_path, str) or not os.path.isfile(audio_path):
        raise ValueError("audioPath must point to an existing audio file")
    if not isinstance(text, str) or not text.strip():
        raise ValueError("text must contain the transcript for this block")
    if not isinstance(language, str) or not language.strip():
        raise ValueError("language is required (use the language actually spoken)")

    audio = whisperx.load_audio(audio_path)
    duration = len(audio) / 16000.0
    model, metadata = get_alignment_model(language.strip().lower())
    segments = [{"start": 0.0, "end": duration, "text": text.strip()}]

    with contextlib.redirect_stdout(sys.stderr):
        aligned = whisperx.align(
            segments,
            model,
            metadata,
            audio,
            get_device(),
            return_char_alignments=False,
        )

    words = []
    for item in aligned.get("word_segments", []):
        start = item.get("start")
        end = item.get("end")
        word = item.get("word")
        if start is None or end is None or not isinstance(word, str):
            continue
        start_ms = max(0, round(float(start) * 1000))
        end_ms = max(start_ms, round(float(end) * 1000))
        if end_ms <= start_ms:
            continue
        words.append({
            "text": word,
            "startMs": start_ms,
            "endMs": end_ms,
            "score": item.get("score"),
        })

    return {
        "requestId": request_id,
        "status": "completed",
        "language": language.strip().lower(),
        "device": get_device(),
        "words": words,
    }


def main():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        request_id = None
        try:
            request = json.loads(line)
            request_id = request.get("requestId")
            emit(align_block(request))
        except Exception as error:
            emit({
                "requestId": request_id,
                "status": "error",
                "error": str(error),
            })


if __name__ == "__main__":
    main()
