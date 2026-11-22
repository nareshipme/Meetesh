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
 *   ?mock=nogpu      the app was built without a GPU backend
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

const CATALOG = [
  { id: "whisper-tiny.en", role: "asr", name: "Whisper Tiny (English)", description: "Fastest, lowest accuracy.", size: 77_704_715, license: "MIT" },
  { id: "whisper-base.en", role: "asr", name: "Whisper Base (English)", description: "Good balance of speed and accuracy for most laptops.", size: 147_964_211, license: "MIT" },
  { id: "whisper-small.en-q5_1", role: "asr", name: "Whisper Small (English, compact)", description: "More accurate than Base, about twice as slow.", size: 190_098_681, license: "MIT" },
  { id: "parakeet-tdt-0.6b-v3-int8", role: "asr", name: "Parakeet TDT 0.6B v3 (25 European languages)", description: "NVIDIA's speech model: fast, writes punctuation, detects the language. On our synthetic test speech it makes fewer errors than Whisper Base on normal audio, but did worse on very faint, distant microphones. 670 MB. Does not cover Hindi or other Indian languages.", size: 670_479_942, license: "CC-BY-4.0" },
  { id: "gemma-3-1b-it", role: "assist", name: "Gemma 3 1B (fast)", description: "About 1.5 times faster and under 1 GB of memory, but hints are more generic.", size: 806_058_272, license: "Gemma terms" },
  { id: "gemma-4-e2b-it", role: "assist", name: "Gemma 4 E2B", description: "The default: good hints, needs about 4 GB of memory.", size: 3_106_738_272, license: "Apache-2.0" },
  { id: "gemma-3-4b-it", role: "assist", name: "Gemma 3 4B", description: "Larger and about 2 times slower than Gemma 4 E2B; offered as an alternative.", size: 2_489_894_016, license: "Gemma terms" },
] as const;

export function installMock(variant: string) {
  let nextId = 3;
  let recordingId: number | null = null;
  let modelInstalled = variant !== "nomodel";
  let assistInstalled = !["nomodel", "noassist", "partial", "dlfail"].includes(variant);
  let hintBusy = false;
  const extraInstalled = new Set<string>();
  const sel = { asr: "whisper-base.en", assist: "gemma-4-e2b-it", summary: "" };
  const isInstalled = (id: string) => (id === "whisper-base.en" ? modelInstalled : id === "gemma-4-e2b-it" ? assistInstalled : extraInstalled.has(id));
  const selectedId = (slot: "asr" | "assist" | "summary") => (slot === "summary" ? sel.summary || sel.assist : sel[slot]);
  const currentSelection = () => ({ asr: sel.asr, assist: sel.assist, summary: sel.summary || sel.assist, summary_follows_hints: sel.summary === "" });
  const ASSIST_ID = "gemma-4-e2b-it";
  const SIZES: Record<string, number> = Object.fromEntries(CATALOG.map((c) => [c.id, c.size]));
  const partial: Record<string, number> = Object.fromEntries(CATALOG.map((c) => [c.id, c.id === ASSIST_ID && variant === "partial" ? Math.round(3_106_738_272 * 0.42) : 0]));
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
          meeting(1, "Meeting on Fri 20 Nov, 4:40 PM", 3600 * 30, { duration_ms: 1_805_000, status: "recovered", audio_dir: "/tmp/1" }),
        ];
  transcripts.set(
    2,
    SAMPLE_LINES.map((text, i) => ({ start_ms: 2000 + i * 7000, end_ms: 6000 + i * 7000, text, source: i % 3 === 1 ? "mic" : "system" })),
  );

  const find = (id: number) => meetings.find((m) => m.id === id)!;

  function startRecordingSim(id: number, withSystem: boolean) {
    recordingId = id;
    startPointsSim(id);
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
  let livePointsOn = true;
  const POINT_STEPS: Array<{ now: string; points: string[] }> = [
    { now: "Discussing the spring campaign budget", points: ["Marketing wants to spend sixty thousand", "The cap from last quarter was forty thousand", "Open: Sam to get a second quote by Wednesday"] },
    { now: "Discussing the launch date", points: ["Open: Sam to get a second quote by Wednesday", "Decided: launch on the twenty second", "Priya confirms with support by Friday"] },
    { now: "Who owns the pricing page?", points: ["Decided: launch on the twenty second", "Open: nobody owns the pricing page since Dana left"] },
  ];
  let pointsTimers: number[] = [];
  const pointsEvent = (id: number, step: number, updating: boolean) =>
    emit("live-points", { meeting_id: id, enabled: livePointsOn, now: step >= 0 ? POINT_STEPS[step].now : "", points: step >= 0 ? POINT_STEPS[step].points : [], updating });
  function startPointsSim(id: number) {
    pointsTimers.forEach((t) => clearTimeout(t));
    pointsTimers = [];
    for (let i = 0; i < POINT_STEPS.length; i++) {
      pointsTimers.push(window.setTimeout(() => livePointsOn && pointsEvent(id, i, true), 5000 + i * 7000));
      pointsTimers.push(window.setTimeout(() => livePointsOn && pointsEvent(id, i, false), 6500 + i * 7000));
    }
  }
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
        emit("hint-done", { sources: [], trigger, meeting_id: meetingId, hint_id: id, text: null, cancelled: true, error: null, first_token_ms: 0, total_ms: 0, tokens: 0, on_gpu: false, cpu_fallback: false });
        return;
      }
      i++;
      emit("hint-piece", { trigger, meeting_id: meetingId, hint_id: id, text: words.slice(0, i).join(" ") });
      if (i >= words.length) {
        clearInterval(timer);
        hintBusy = false;
        emit("hint-done", { sources: meetingId === 2 ? [] : [{ meeting_id: 2, title: "Weekly product sync", day: "Mon 16 Nov" }], trigger, meeting_id: meetingId, hint_id: id, text: words.join(" "), cancelled: false, error: null, first_token_ms: 900, total_ms: Date.now() - t0, tokens: words.length, on_gpu: true, cpu_fallback: false });
      }
    }, 450);
    return id;
  }

  // Meeting notes: a line of jobs, written one at a time; paused (not cancelled) while a hint runs or a recording is going.
  const notesLine: number[] = [];
  let notesRunning: number | null = null;
  let notesStep = 0;
  let notesTimer: number | undefined;
  const NOTES_STEPS = 3;
  const evt = (id: number, state: string, extra: Record<string, unknown> = {}) =>
    emit("summary-progress", { meeting_id: id, state, position: 0, paused_for: null, done: notesStep, total: NOTES_STEPS, finished: state === "finished", cancelled: false, error: null, ...extra });
  const pausedFor = (): "hint" | "recording" | null => (hintBusy ? "hint" : recordingId !== null ? "recording" : null);
  let lastPaused: string | null = null;
  function pumpNotes() {
    if (notesRunning === null) {
      const next = notesLine.shift();
      if (next === undefined) return;
      notesRunning = next;
      notesStep = 0;
      lastPaused = null;
      evt(next, "running");
      notesTimer = window.setInterval(() => {
        const why = pausedFor();
        if (why !== lastPaused) {
          lastPaused = why;
          evt(notesRunning!, why ? "paused" : "running", { paused_for: why });
        }
        if (why) return; // waiting, not cancelled
        notesStep++;
        const id = notesRunning!;
        if (notesStep < NOTES_STEPS) return void evt(id, "running");
        clearInterval(notesTimer);
        notesRunning = null;
        if (variant === "sumfail") evt(id, "finished", { error: "The model did not return usable meeting notes. Try again." });
        else {
          const m = find(id);
          m.summary = SAMPLE_NOTES;
          m.summary_model = "gemma-4-e2b-it";
          if (m.title_source !== "user") {
            m.title = "Onboarding launch date";
            m.title_source = "model";
          }
          evt(id, "finished", { done: NOTES_STEPS });
        }
        pumpNotes();
      }, 1200);
    }
  }
  function queueNotes(id: number) {
    if (notesRunning === id || notesLine.includes(id)) throw "Notes for this meeting are already being written or waiting their turn";
    notesLine.push(id);
    evt(id, "queued", { position: notesLine.length - 1 + (notesRunning !== null ? 1 : 0) });
    pumpNotes();
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
    if (modelInstalled && assistInstalled) window.setTimeout(() => queueNotes(id), 300);
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
        window.setTimeout(() => publish(id, "ready", { backend: variant === "gpufail" ? "CPU (GPU failed)" : variant === "nogpu" ? "CPU (no GPU support in this build)" : "GPU" }), 1200);
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
          const now = new Date();
          const wd = now.toLocaleDateString("en-US", { weekday: "short" });
          const mon = now.toLocaleDateString("en-US", { month: "short" });
          const m = meeting(nextId++, `Meeting on ${wd} ${now.getDate()} ${mon}, ${now.toLocaleTimeString("en-US", { hour: "numeric", minute: "2-digit" })}`, 0);
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
        case "search_meetings": {
          const words = String(a.query).toLowerCase().split(/[^\p{L}\p{N}]+/u).filter(Boolean);
          const mark = (text: string) => text.replace(new RegExp(`(${words.map((w) => w.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")).join("|")})`, "gi"), "\u0001$1\u0002");
          const has = (text: string) => words.every((w) => text.toLowerCase().includes(w));
          const out: any[] = [];
          for (const m of [...meetings].sort((x, y) => y.id - x.id)) {
            const matches: any[] = [];
            if (has(m.title)) matches.push({ kind: "title", snippet: mark(m.title), start_ms: null });
            if (m.summary && has(m.summary)) matches.push({ kind: "notes", snippet: mark(m.summary.replace(/\n+/g, " ").slice(0, 120)), start_ms: null });
            for (const s of transcripts.get(m.id) ?? []) if (has(s.text) && matches.length < 3) matches.push({ kind: "transcript", snippet: mark(s.text), start_ms: s.start_ms });
            if (words.length && matches.length) out.push({ meeting_id: m.id, title: m.title, created_at: m.created_at, matches });
          }
          return out;
        }
        case "meeting_markdown":
          return `# ${find(a.id).title}\n\n*sample*\n`;
        case "export_markdown":
          return `/Users/me/Library/Application Support/app.meetesh.desktop/exports/${a.id}-meeting.md`;
        case "summarize_meeting":
          queueNotes(a.id);
          return null;
        case "cancel_summary": {
          if (notesRunning === a.id) {
            clearInterval(notesTimer);
            notesRunning = null;
            emit("summary-progress", { meeting_id: a.id, state: "finished", position: 0, paused_for: null, done: 0, total: NOTES_STEPS, finished: true, cancelled: true, error: null });
            pumpNotes();
          } else if (notesLine.includes(a.id)) {
            notesLine.splice(notesLine.indexOf(a.id), 1);
            emit("summary-progress", { meeting_id: a.id, state: "finished", position: 0, paused_for: null, done: 0, total: NOTES_STEPS, finished: true, cancelled: true, error: null });
          }
          return null;
        }
        case "summary_status":
          if (notesRunning === a.id) return { state: pausedFor() ? "paused" : "running", position: 0, paused_for: pausedFor() };
          if (notesLine.includes(a.id)) return { state: "queued", position: notesLine.indexOf(a.id) + (notesRunning !== null ? 1 : 0), paused_for: null };
          return { state: "idle", position: 0, paused_for: null };
        case "clear_summary":
          find(a.id).summary = "";
          find(a.id).summary_model = "";
          return find(a.id);
        case "export_audio":
          return "/Users/me/Library/Application Support/app.meetesh.desktop/recordings/2/meeting.wav";
        case "list_models":
          return CATALOG.map((c) => ({
            id: c.id,
            role: c.role,
            name: c.name,
            description: c.description,
            size_bytes: c.size,
            license: c.license,
            installed: isInstalled(c.id),
            partial_bytes: partial[c.id] ?? 0,
            is_default: c.id === "whisper-base.en" || c.id === ASSIST_ID,
            selected_for: (["asr", "assist", "summary"] as const).filter((slot) => selectedId(slot) === c.id),
          }));
        case "model_selection":
          return currentSelection();
        case "select_model": {
          if (a.slot === "summary" && a.id === "") sel.summary = "";
          else sel[a.slot as "asr" | "assist" | "summary"] = a.id;
          if (a.id && isInstalled(a.id) && a.slot !== "summary") startLoad(a.id);
          return currentSelection();
        }
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
        case "get_live_points":
          return { meeting_id: recordingId ?? 0, enabled: livePointsOn, now: "", points: [], updating: false };
        case "set_live_points":
          livePointsOn = !!a.enabled;
          emit("live-points", { meeting_id: recordingId ?? 0, enabled: livePointsOn, now: "", points: [], updating: false });
          return null;
        case "get_auto_hints":
          return autoHints;
        case "set_auto_hints":
          autoHints = a.mode;
          return null;
        case "download_model": {
          const id: string = a.id;
          const total = SIZES[id];
          const step = SIZES[id] > 1e9 ? 90_000_000 : 12_000_000;
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
                else if (id === "whisper-base.en") modelInstalled = true;
                else extraInstalled.add(id);
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
