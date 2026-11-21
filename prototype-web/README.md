# Meetesh (web prototype)

A minimal, local-first meeting notes app (Meetily-style): record, transcribe, summarize.

## Features (v0.1)
- Record microphone audio in the browser; audio is sent in 15s chunks and transcribed live
- Local transcription with faster-whisper (no cloud needed)
- Meetings stored in SQLite (`meetings.db`)
- Summary (key points, decisions, action items) via local Ollama, or Claude if `ANTHROPIC_API_KEY` is set
- Edit title/transcript, delete meetings

## Run
Requires Python 3.10+ and ffmpeg.

```bash
pip install -r requirements.txt
uvicorn app:app --port 8000
```

Open http://localhost:8000 (mic access needs localhost or HTTPS).

## Config (env vars)
| Var | Default |
|---|---|
| `WHISPER_MODEL` | `base` (try `small`, `medium`) |
| `OLLAMA_URL` / `OLLAMA_MODEL` | `http://localhost:11434` / `llama3.2` |
| `ANTHROPIC_API_KEY` / `ANTHROPIC_MODEL` | unset / `claude-haiku-5-5` |
| `MEETSCRIBE_DB` | `./meetings.db` |

## Not yet (roadmap)
System/speaker audio capture (needs a desktop wrapper such as Tauri), speaker diarization, search, export, import of audio files, desktop packaging.
