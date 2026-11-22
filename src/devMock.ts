/**
 * Development-only fake backend so the UI can be developed and screenshotted in a plain browser
 * (no Tauri, no microphone, no model). Open http://localhost:1420/?mock after `npm run dev`.
 *
 *   ?mock            two sample meetings, speech model installed
 *   ?mock=empty      no meetings
 *   ?mock=nomodel    neither model downloaded
 *   ?mock=noassist   speech model installed, live-assist model not downloaded
 *   ?mock=partial    live-assist download was paused at 42% earlier
 *   ?mock=loading    models installed; live assist stays stuck at "Loading 42%"
 *   ?mock=loadfail   live assist fails to load until you press Retry
 *   ?mock=gpufail    live assist loads, but the GPU failed so the CPU is used
 *   ?mock=nosystem   system-audio capture unavailable (like Linux)
 *   ?mock=sysfail    system-audio capture is asked for but fails (permission denied)
 *   ?mock=sumfail    writing the meeting notes fails
 *   ?mock=dlfail     the first live-assist download fails at 40% (Retry works)
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

const SAMPLE_NOTES = `## Summary
The team talked through the plan for Meetesh: live transcription, hints from an on-device model, and replaying and copying the transcript.

## Decisions
- Keep everything on the device
- Ship live transcription before summaries

## Action items
- Sam writes the release notes by Thursday
- Check the transcript on a real call`;

export function installMock(variant: string) {
  let nextId = 3;
  let recordingId: number | null = null;
  let modelInstalled = variant !== "nomodel";
  let assistInstalled = !["nomodel", "noassist", "partial", "dlfail"].includes(variant);
  let hintBusy = false;
  const ASSIST_ID = "gemma-4-e2b-it";
  const SIZES: Record<string, number> = { "whisper-base.en": 147_964_211, [ASSIST_ID]: 3_106_738_272 };
  const partial: Record<string, number> = { "whisper-base.en": 0, [ASSIST_ID]: variant === "partial" ? Math.round(3_106_738_272 * 0.42) : 0 };
  const runtime: Record<string, any> = {};
  let dlTimer: number | undefined;
  let dlFailedOnce = false;
  let loadFailedOnce = false;
  let timers: number[] = [];
  const transcripts = new Map<number, TranscriptSegment[]>();

  const meeting = (id: number, title: string, ageSec: number, extra: Partial<Meeting> = {}): Meeting => ({
    id,
    title,
    created_at: now() - ageSec,
    summary: "",
    summary_model: "",
    title_source: "default",
    transcript_model: "",
    agenda: "",
    status: "stopped",
    duration_ms: 0,
    audio_dir: "",
    has_system_audio: false,
    ...extra,
  });

  const meetings: Meeting[] =
    variant === "empty"
      ? []
      : [
          meeting(2, "Weekly product sync", 3600 * 3, { duration_ms: 52_000, title_source: "user", transcript_model: "whisper-base.en", audio_dir: "/tmp/2", has_system_audio: true, summary: SAMPLE_NOTES, summary_model: "gemma-4-e2b-it" }),
          meeting(1, "Meeting 2026-11-20 16:40", 3600 * 30, { duration_ms: 1_805_000, status: "recovered", audio_dir: "/tmp/1" }),
        ];
  transcripts.set(
    2,
    SAMPLE_LINES.map((text, i) => ({ start_ms: 2000 + i * 7000, end_ms: 6000 + i * 7000, text, source: i % 3 === 1 ? "mic" : "system" })),
  );

  const find = (id: number) => meetings.find((m) => m.id === id)!;

  function startRecordingSim(id: number, withSystem: boolean) {
    recordingId = id;
    find(id).has_system_audio = withSystem;
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
        const seg = { start_ms: Date.now() - t0, end_ms: Date.now() - t0 + 2000, text: SAMPLE_LINES[line++ % SAMPLE_LINES.length], source: (withSystem && line % 2 === 0 ? "system" : "mic") as "mic" | "system" };
        // Every fourth line is a question from the other side.
        if (withSystem && line % 4 === 0) {
          seg.text = "Can you tell us when the release will be ready?";
          seg.source = "system";
        }
        transcripts.get(id)!.push(seg);
        if (modelInstalled) emit("live-segments", { meeting_id: id, segments: [seg] });
        if (assistInstalled && withSystem && autoHints !== "off" && seg.source === "system" && seg.text.endsWith("?")) {
          window.setTimeout(() => {
            try {
              runHintSim(id, "question");
            } catch {
              /* busy: skipped, like the app */
            }
          }, 1600);
        }
      }, 2500),
    );
  }

  let autoHints = "off";
  function runHintSim(meetingId: number, trigger: "question" | "pause" | null): number {
    if (hintBusy) throw "A hint is already being generated";
    hintBusy = true;
    const words = "Can you clarify what support needs that extra week for?".split(" ");
    let i = 0;
    const t0 = Date.now();
    const id = Date.now();
    const timer = window.setInterval(() => {
      if (!hintBusy) {
        clearInterval(timer);
        emit("hint-done", { trigger, meeting_id: meetingId, hint_id: id, text: null, cancelled: true, error: null, first_token_ms: 0, total_ms: 0, tokens: 0, on_gpu: false, cpu_fallback: false });
        return;
      }
      i++;
      emit("hint-piece", { trigger, meeting_id: meetingId, hint_id: id, text: words.slice(0, i).join(" ") });
      if (i >= words.length) {
        clearInterval(timer);
        hintBusy = false;
        emit("hint-done", { trigger, meeting_id: meetingId, hint_id: id, text: words.join(" "), cancelled: false, error: null, first_token_ms: 900, total_ms: Date.now() - t0, tokens: words.length, on_gpu: true, cpu_fallback: false });
      }
    }, 450);
    return id;
  }

  let summaryTimer: number | undefined;
  let summaryFor: number | null = null;
  function startSummarySim(id: number) {
    summaryFor = id;
    let step = 0;
    const total = 3;
    emit("summary-progress", { meeting_id: id, done: 0, total, finished: false, cancelled: false, error: null });
    summaryTimer = window.setInterval(() => {
      step++;
      if (step < total) return void emit("summary-progress", { meeting_id: id, done: step, total, finished: false, cancelled: false, error: null });
      clearInterval(summaryTimer);
      summaryFor = null;
      if (variant === "sumfail") return void emit("summary-progress", { meeting_id: id, done: 0, total, finished: true, cancelled: false, error: "The model did not return usable meeting notes. Try again." });
      const m = find(id);
      m.summary = SAMPLE_NOTES;
      m.summary_model = "gemma-4-e2b-it";
      if (m.title_source !== "user") {
        m.title = "Onboarding launch date";
        m.title_source = "model";
      }
      emit("summary-progress", { meeting_id: id, done: total, total, finished: true, cancelled: false, error: null });
    }, 1200);
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
    // Like the app: notes are written by themselves once the transcript is complete.
    if (modelInstalled && assistInstalled) window.setTimeout(() => startSummarySim(id), 300);
    return m;
  }

  const publish = (id: string, state: string, extra: Record<string, unknown> = {}) => {
    const role = id === ASSIST_ID ? "assist" : "asr";
    runtime[id] = { id, role, state, progress: null, backend: null, error: null, ...extra };
    emit("model-load", runtime[id]);
  };

  /** Fake loading: real-looking progress for the live-assist model, indeterminate for speech. */
  function startLoad(id: string) {
    publish(id, "loading", id === ASSIST_ID ? { progress: 0 } : {});
    let p = 0;
    const t = window.setInterval(() => {
      if (id !== ASSIST_ID) {
        clearInterval(t);
        publish(id, "warming");
        window.setTimeout(() => publish(id, "ready", { backend: "CPU" }), 600);
        return;
      }
      p += 0.07;
      if (variant === "loading" && p >= 0.42) {
        clearInterval(t);
        publish(id, "loading", { progress: 0.42 });
        return;
      }
      if (variant === "loadfail" && !loadFailedOnce && p >= 0.3) {
        clearInterval(t);
        loadFailedOnce = true;
        publish(id, "failed", { error: "llama could not load model: out of memory" });
        return;
      }
      if (p >= 1) {
        clearInterval(t);
        publish(id, "warming", { progress: 1 });
        window.setTimeout(() => publish(id, "ready", { backend: variant === "gpufail" ? "CPU (GPU failed)" : "GPU" }), 1200);
      } else publish(id, "loading", { progress: p });
    }, 250);
  }
  window.setTimeout(() => {
    if (modelInstalled) startLoad("whisper-base.en");
    if (assistInstalled) startLoad(ASSIST_ID);
  }, 400);

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
          find(a.id).title_source = "user";
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
          if (a.systemAudio && variant === "sysfail") {
            startRecordingSim(a.meetingId, false);
            return { meeting: find(a.meetingId), system_audio_error: "Could not capture system audio: permission denied." };
          }
          startRecordingSim(a.meetingId, !!a.systemAudio);
          return { meeting: find(a.meetingId), system_audio_error: null };
        case "system_audio_support":
          return variant === "nosystem"
            ? { available: false, reason: "Capturing the other participants is not available on Linux yet. Only your microphone is recorded." }
            : { available: true, reason: "" };
        case "stop_recording":
          return stopRecordingSim();
        case "summarize_meeting":
          startSummarySim(a.id);
          return null;
        case "cancel_summary":
          if (summaryFor !== null) {
            clearInterval(summaryTimer);
            emit("summary-progress", { meeting_id: summaryFor, done: 0, total: 3, finished: true, cancelled: true, error: null });
            summaryFor = null;
          }
          return null;
        case "active_summary":
          return summaryFor;
        case "clear_summary":
          find(a.id).summary = "";
          find(a.id).summary_model = "";
          return find(a.id);
        case "export_audio":
          return "/Users/me/Library/Application Support/app.meetesh.desktop/recordings/2/meeting.wav";
        case "list_models":
          return [
            {
              id: "whisper-base.en",
              role: "asr",
              name: "Whisper Base (English)",
              description: "",
              size_bytes: 147_964_211,
              license: "MIT",
              installed: modelInstalled,
              partial_bytes: partial["whisper-base.en"],
              is_default: true,
            },
            {
              id: "gemma-4-e2b-it",
              role: "assist",
              name: "Gemma 4 E2B (live assist)",
              description: "",
              size_bytes: 3_106_738_272,
              license: "Apache-2.0",
              installed: assistInstalled,
              partial_bytes: partial[ASSIST_ID],
              is_default: true,
            },
          ];
        case "set_agenda":
          find(a.id).agenda = a.agenda;
          return find(a.id);
        case "assist_ready":
          return assistInstalled;
        case "active_hint":
          return null;
        case "cancel_hint":
          hintBusy = false;
          return null;
        case "request_hint":
          return runHintSim(a.meetingId, null);
        case "get_auto_hints":
          return autoHints;
        case "set_auto_hints":
          autoHints = a.mode;
          return null;
        case "download_model": {
          const id: string = a.id;
          const total = SIZES[id];
          const step = id === ASSIST_ID ? 90_000_000 : 12_000_000;
          const failAt = variant === "dlfail" && id === ASSIST_ID && !dlFailedOnce ? total * 0.4 : Infinity;
          dlTimer = window.setInterval(() => {
            partial[id] = Math.min(total, partial[id] + step);
            if (partial[id] >= failAt) {
              clearInterval(dlTimer);
              dlFailedOnce = true;
              emit("model-download", { id, phase: "failed", downloaded: partial[id], total, error: "download interrupted: connection reset by peer" });
              return;
            }
            if (partial[id] >= total) {
              clearInterval(dlTimer);
              emit("model-download", { id, phase: "verifying", downloaded: total, total, error: null });
              window.setTimeout(() => {
                partial[id] = 0;
                if (id === ASSIST_ID) assistInstalled = true;
                else modelInstalled = true;
                emit("model-download", { id, phase: "done", downloaded: total, total, error: null });
                startLoad(id);
              }, 1500);
            } else emit("model-download", { id, phase: "downloading", downloaded: partial[id], total, error: null });
          }, 250);
          return null;
        }
        case "cancel_download": {
          clearInterval(dlTimer);
          const id = Object.keys(partial).find((k) => partial[k] > 0 && partial[k] < SIZES[k]);
          if (id) emit("model-download", { id, phase: "paused", downloaded: partial[id], total: SIZES[id], error: null });
          return null;
        }
        case "discard_download":
          partial[a.id] = 0;
          return null;
        case "log_dir":
          return "/Users/me/Library/Logs/app.meetesh.desktop";
        case "model_runtime":
          return Object.values(runtime);
        case "retry_model_load":
          startLoad(a.id);
          return null;
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
