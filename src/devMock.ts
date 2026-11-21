/**
 * Development-only fake backend so the UI can be developed and screenshotted in a plain browser
 * (no Tauri, no microphone, no model). Open http://localhost:1420/?mock after `npm run dev`.
 *
 *   ?mock            two sample meetings, speech model installed
 *   ?mock=empty      no meetings
 *   ?mock=nomodel    speech model not downloaded
 *
 * Never bundled into production builds: main.tsx imports it only when `import.meta.env.DEV`.
 */
import { emit } from "@tauri-apps/api/event";
import { mockIPC } from "@tauri-apps/api/mocks";
import type { Meeting, TranscriptSegment } from "./api";

const now = () => Math.floor(Date.now() / 1000);

const SAMPLE_LINES = [
  "Okay, hello.",
  "I'm going to join this meeting and I would like to",
  "talk more about the certain topic that I am looking for.",
  "Let's discuss the topic. Today I'm building Meetesh.",
  "It has live transcription and then it also gives you hints.",
  "You can replay the audio and copy the transcript.",
];

export function installMock(variant: string) {
  let nextId = 3;
  let recordingId: number | null = null;
  let modelInstalled = variant !== "nomodel";
  let timers: number[] = [];
  const transcripts = new Map<number, TranscriptSegment[]>();

  const meeting = (id: number, title: string, ageSec: number, extra: Partial<Meeting> = {}): Meeting => ({
    id,
    title,
    created_at: now() - ageSec,
    summary: "",
    transcript_model: "",
    status: "stopped",
    duration_ms: 0,
    audio_dir: "",
    ...extra,
  });

  const meetings: Meeting[] =
    variant === "empty"
      ? []
      : [
          meeting(2, "Weekly product sync", 3600 * 3, { duration_ms: 52_000, transcript_model: "whisper-base.en", audio_dir: "/tmp/2" }),
          meeting(1, "Meeting 2026-11-20 16:40", 3600 * 30, { duration_ms: 1_805_000, status: "recovered", audio_dir: "/tmp/1" }),
        ];
  transcripts.set(
    2,
    SAMPLE_LINES.map((text, i) => ({ start_ms: 2000 + i * 7000, end_ms: 6000 + i * 7000, text })),
  );

  const find = (id: number) => meetings.find((m) => m.id === id)!;

  function startRecordingSim(id: number) {
    recordingId = id;
    find(id).status = "recording";
    const t0 = Date.now();
    let line = 0;
    transcripts.set(id, []);
    timers.push(
      window.setInterval(() => {
        const speaking = Math.floor((Date.now() - t0) / 1500) % 3 !== 2;
        const rms = speaking ? 0.01 + Math.random() * 0.06 : 0.0008;
        emit("recording-level", rms);
      }, 100),
      window.setInterval(() => {
        const seg = { start_ms: Date.now() - t0, end_ms: Date.now() - t0 + 2000, text: SAMPLE_LINES[line++ % SAMPLE_LINES.length] };
        transcripts.get(id)!.push(seg);
        if (modelInstalled) emit("live-segments", { meeting_id: id, segments: [seg] });
      }, 2500),
    );
  }

  function stopRecordingSim() {
    timers.forEach((t) => clearInterval(t));
    timers = [];
    const m = find(recordingId!);
    m.status = "stopped";
    m.duration_ms = 30_000;
    m.audio_dir = "/tmp/x";
    if (modelInstalled) m.transcript_model = "whisper-base.en";
    const id = recordingId!;
    recordingId = null;
    emit("live-done", { meeting_id: id, full_pass_started: false });
    return m;
  }

  mockIPC(
    (cmd, args) => {
      const a = (args ?? {}) as Record<string, any>;
      switch (cmd) {
        case "list_meetings":
          return meetings.map((m) => ({ ...m })).sort((x, y) => y.id - x.id);
        case "create_meeting": {
          const m = meeting(nextId++, `Meeting ${new Date().toISOString().slice(0, 16).replace("T", " ")}`, 0);
          meetings.push(m);
          return m;
        }
        case "rename_meeting":
          find(a.id).title = a.title;
          return find(a.id);
        case "delete_meeting":
          meetings.splice(meetings.findIndex((m) => m.id === a.id), 1);
          return null;
        case "list_input_devices":
          return [
            { name: "MacBook Pro Microphone", is_default: true },
            { name: "External USB Microphone", is_default: false },
          ];
        case "active_recording":
          return recordingId;
        case "start_recording":
          startRecordingSim(a.meetingId);
          return find(a.meetingId);
        case "stop_recording":
          return stopRecordingSim();
        case "export_audio":
          return "/Users/me/Library/Application Support/app.meetesh.desktop/recordings/2/meeting.wav";
        case "list_models":
          return [
            {
              id: "whisper-base.en",
              name: "Whisper Base (English)",
              description: "",
              size_bytes: 147_964_211,
              license: "MIT",
              installed: modelInstalled,
              is_default: true,
            },
          ];
        case "download_model": {
          let d = 0;
          const total = 147_964_211;
          const t = window.setInterval(() => {
            d += 9_000_000;
            if (d >= total) {
              clearInterval(t);
              modelInstalled = true;
              emit("model-download", { id: a.id, downloaded: total, total, done: true, error: null });
            } else emit("model-download", { id: a.id, downloaded: d, total, done: false, error: null });
          }, 300);
          return null;
        }
        case "get_transcript":
          return transcripts.get(a.id) ?? [];
        case "transcribe_meeting": {
          let p = 0;
          const t = window.setInterval(() => {
            p += 0.1;
            if (p >= 1) {
              clearInterval(t);
              emit("transcribe-progress", { meeting_id: a.id, progress: 1, done: true, cancelled: false, error: null });
            } else emit("transcribe-progress", { meeting_id: a.id, progress: p, done: false, cancelled: false, error: null });
          }, 400);
          return null;
        }
        case "active_transcription":
          return null;
        default:
          return null; // cancel_download, cancel_transcription, delete_model, ...
      }
    },
    { shouldMockEvents: true },
  );
}
