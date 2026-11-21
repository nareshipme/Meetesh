"""Meetesh (web prototype): a minimal, local-first meeting notes app.

Record in the browser -> transcribe locally (faster-whisper) -> summarize (Ollama or Claude).
"""
import os
import sqlite3
import subprocess
import tempfile
import time
from pathlib import Path

import httpx
from fastapi import FastAPI, File, HTTPException, UploadFile
from fastapi.responses import FileResponse
from fastapi.staticfiles import StaticFiles
from pydantic import BaseModel

BASE = Path(__file__).parent
DB_PATH = os.environ.get("MEETSCRIBE_DB", str(BASE / "meetings.db"))
WHISPER_MODEL = os.environ.get("WHISPER_MODEL", "base")
OLLAMA_URL = os.environ.get("OLLAMA_URL", "http://localhost:11434")
OLLAMA_MODEL = os.environ.get("OLLAMA_MODEL", "llama3.2")
ANTHROPIC_API_KEY = os.environ.get("ANTHROPIC_API_KEY")
ANTHROPIC_MODEL = os.environ.get("ANTHROPIC_MODEL", "claude-haiku-5-5")

app = FastAPI(title="Meetesh (web prototype)")
_whisper = None


def db():
    conn = sqlite3.connect(DB_PATH)
    conn.row_factory = sqlite3.Row
    conn.execute(
        """CREATE TABLE IF NOT EXISTS meetings (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            title TEXT NOT NULL,
            created_at REAL NOT NULL,
            transcript TEXT NOT NULL DEFAULT '',
            summary TEXT NOT NULL DEFAULT ''
        )"""
    )
    return conn


def get_whisper():
    global _whisper
    if _whisper is None:
        from faster_whisper import WhisperModel

        _whisper = WhisperModel(WHISPER_MODEL, device="auto", compute_type="int8")
    return _whisper


def transcribe_bytes(data: bytes) -> str:
    # Browser audio is webm/opus; convert to 16k mono wav for whisper.
    with tempfile.TemporaryDirectory() as d:
        src, wav = Path(d) / "in.webm", Path(d) / "out.wav"
        src.write_bytes(data)
        r = subprocess.run(
            ["ffmpeg", "-y", "-i", str(src), "-ar", "16000", "-ac", "1", str(wav)],
            capture_output=True,
        )
        if r.returncode != 0:
            raise HTTPException(400, "Could not decode audio chunk")
        segments, _ = get_whisper().transcribe(str(wav), vad_filter=True)
        return " ".join(s.text.strip() for s in segments).strip()


SUMMARY_PROMPT = (
    "Summarize this meeting transcript in markdown with these sections: "
    "**Summary** (2-4 sentences), **Key points**, **Decisions**, **Action items** "
    "(with owners if mentioned). Only use information from the transcript.\n\n"
    "Transcript:\n{t}"
)


def summarize(transcript: str) -> str:
    prompt = SUMMARY_PROMPT.format(t=transcript)
    if ANTHROPIC_API_KEY:
        r = httpx.post(
            "https://api.anthropic.com/v1/messages",
            headers={"x-api-key": ANTHROPIC_API_KEY, "anthropic-version": "2023-06-01"},
            json={"model": ANTHROPIC_MODEL, "max_tokens": 1500,
                  "messages": [{"role": "user", "content": prompt}]},
            timeout=120,
        )
        r.raise_for_status()
        return r.json()["content"][0]["text"]
    try:
        r = httpx.post(
            f"{OLLAMA_URL}/api/generate",
            json={"model": OLLAMA_MODEL, "prompt": prompt, "stream": False},
            timeout=300,
        )
        r.raise_for_status()
        return r.json()["response"]
    except httpx.HTTPError as e:
        raise HTTPException(
            502, f"No summarizer available. Start Ollama or set ANTHROPIC_API_KEY. ({e})"
        )


class NewMeeting(BaseModel):
    title: str = ""


class Edit(BaseModel):
    title: str | None = None
    transcript: str | None = None


@app.post("/api/meetings")
def create_meeting(body: NewMeeting):
    title = body.title.strip() or time.strftime("Meeting %Y-%m-%d %H:%M")
    with db() as c:
        cur = c.execute("INSERT INTO meetings (title, created_at) VALUES (?, ?)", (title, time.time()))
        return {"id": cur.lastrowid, "title": title}


@app.get("/api/meetings")
def list_meetings():
    with db() as c:
        rows = c.execute("SELECT id, title, created_at, summary != '' AS has_summary FROM meetings ORDER BY id DESC")
        return [dict(r) for r in rows]


@app.get("/api/meetings/{mid}")
def get_meeting(mid: int):
    with db() as c:
        row = c.execute("SELECT * FROM meetings WHERE id=?", (mid,)).fetchone()
    if not row:
        raise HTTPException(404)
    return dict(row)


@app.patch("/api/meetings/{mid}")
def edit_meeting(mid: int, body: Edit):
    with db() as c:
        if body.title is not None:
            c.execute("UPDATE meetings SET title=? WHERE id=?", (body.title, mid))
        if body.transcript is not None:
            c.execute("UPDATE meetings SET transcript=? WHERE id=?", (body.transcript, mid))
    return get_meeting(mid)


@app.delete("/api/meetings/{mid}")
def delete_meeting(mid: int):
    with db() as c:
        c.execute("DELETE FROM meetings WHERE id=?", (mid,))
    return {"ok": True}


@app.post("/api/meetings/{mid}/audio")
async def add_audio(mid: int, file: UploadFile = File(...)):
    """Transcribe one audio chunk and append it to the meeting transcript."""
    text = transcribe_bytes(await file.read())
    with db() as c:
        row = c.execute("SELECT transcript FROM meetings WHERE id=?", (mid,)).fetchone()
        if not row:
            raise HTTPException(404)
        full = (row["transcript"] + " " + text).strip() if text else row["transcript"]
        c.execute("UPDATE meetings SET transcript=? WHERE id=?", (full, mid))
    return {"text": text, "transcript": full}


@app.post("/api/meetings/{mid}/summarize")
def summarize_meeting(mid: int):
    m = get_meeting(mid)
    if not m["transcript"].strip():
        raise HTTPException(400, "Transcript is empty")
    summary = summarize(m["transcript"])
    with db() as c:
        c.execute("UPDATE meetings SET summary=? WHERE id=?", (summary, mid))
    return {"summary": summary}


@app.get("/")
def index():
    return FileResponse(BASE / "static" / "index.html")


app.mount("/static", StaticFiles(directory=BASE / "static"), name="static")
