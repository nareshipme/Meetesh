import { useCallback, useEffect, useRef, useState } from "react";
import { api, AutoHintMode, Meeting, ModelLoadEvent, TranscriptSegment } from "../api";
import type { Notify } from "../hooks/useToasts";
import { useHint } from "../hooks/useHint";
import { errorText, fmtDuration } from "../lib/format";
import HintPanel from "./HintPanel";

interface Props {
  meeting: Meeting;
  modelReady: boolean;
  assistReady: boolean;
  assistLoad: ModelLoadEvent | undefined;
  recording: boolean;
  notify: Notify;
  onChange: () => void;
}

export default function Transcript({ meeting, modelReady, assistReady, assistLoad, recording, notify, onChange }: Props) {
  const [segments, setSegments] = useState<TranscriptSegment[]>([]);
  const [progress, setProgress] = useState<number | null>(null);
  const bodyRef = useRef<HTMLDivElement>(null);
  const stickToBottom = useRef(true);
  const hint = useHint(meeting.id, notify);
  const [autoMode, setAutoMode] = useState<AutoHintMode>("off");
  useEffect(() => {
    api.getAutoHints().then(setAutoMode).catch(() => {});
  }, []);
  async function chooseAutoMode(mode: AutoHintMode) {
    const before = autoMode;
    setAutoMode(mode);
    try {
      await api.setAutoHints(mode);
    } catch (e) {
      setAutoMode(before);
      notify("error", errorText(e));
    }
  }
  const assistLoading = assistLoad?.state === "loading" || assistLoad?.state === "warming";
  const canHint = assistReady && !assistLoading && segments.length > 0 && hint.state.kind !== "working";

  // Cmd/Ctrl+Shift+H asks for a hint (in-app shortcut; a global hotkey comes later).
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if ((e.metaKey || e.ctrlKey) && e.shiftKey && e.key.toLowerCase() === "h" && canHint) {
        e.preventDefault();
        hint.request();
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [canHint, hint.request]);

  const load = useCallback(() => {
    api.getTranscript(meeting.id).then(setSegments).catch((e) => notify("error", errorText(e)));
  }, [meeting.id, notify]);

  useEffect(() => {
    load();
    api.activeTranscription().then((id) => setProgress(id === meeting.id ? 0 : null)).catch(() => {});
    const unProgress = api.onTranscribe((e) => {
      if (e.meeting_id !== meeting.id) return;
      if (e.done) {
        setProgress(null);
        if (e.error) notify("error", e.error);
        load();
        onChange();
      } else {
        setProgress(e.progress);
      }
    });
    // Live transcription: text arrives while the meeting is being recorded.
    const unLive = api.onLiveSegments((e) => {
      if (e.meeting_id === meeting.id) setSegments((prev) => [...prev, ...e.segments]);
    });
    const unDone = api.onLiveDone((e) => {
      if (e.meeting_id !== meeting.id) return;
      load(); // pick up the result of a gap-filling pass, if one ran
      onChange();
    });
    return () => {
      unProgress.then((f) => f());
      unLive.then((f) => f());
      unDone.then((f) => f());
    };
  }, [meeting.id, load, notify, onChange]);

  // Follow new live text, but only while the reader is already at the bottom.
  useEffect(() => {
    const el = bodyRef.current;
    if (el && recording && stickToBottom.current) el.scrollTop = el.scrollHeight;
  }, [segments, recording]);

  function onScroll() {
    const el = bodyRef.current;
    if (el) stickToBottom.current = el.scrollHeight - el.scrollTop - el.clientHeight < 80;
  }

  async function run() {
    try {
      setProgress(0);
      await api.transcribeMeeting(meeting.id);
    } catch (e) {
      setProgress(null);
      notify("error", errorText(e));
    }
  }

  async function copy() {
    const text = segments.map((s) => `${fmtDuration(s.start_ms)}  ${labelled ? `${who(s)}: ` : ""}${s.text}`).join("\n");
    try {
      await navigator.clipboard.writeText(text);
      notify("info", "Transcript copied");
    } catch {
      notify("error", "Could not copy to the clipboard");
    }
  }

  // "You" / "Others" only matter when both sides of a call were recorded.
  const labelled = meeting.has_system_audio || segments.some((s) => s.source === "system");
  const who = (s: TranscriptSegment) => (s.source === "system" ? "Others" : "You");
  const hasAudio = meeting.duration_ms > 0 && !recording;
  const transcribing = progress !== null;

  return (
    <section className="transcript">
      <div className="transcript-head">
        <h2>
          Transcript
          {recording && modelReady && <span className="live-badge">Live</span>}
        </h2>
        <div className="row">
          {(
            <select
              className="auto-hints"
              aria-label="Automatic hints"
              value={autoMode}
              disabled={!assistReady}
              onChange={(e) => chooseAutoMode(e.target.value as AutoHintMode)}
              title={
                assistReady
                  ? "Offer a hint by itself. Questions are noticed when the other participants are recorded (system audio); pauses work with a microphone alone."
                  : "Download the live assist model first (sidebar)"
              }
            >
              <option value="off">Auto hints: off</option>
              <option value="questions">Auto hints: on questions</option>
              <option value="questions_and_pauses">Auto hints: questions and pauses</option>
            </select>
          )}
          <button
            className="btn btn-sm btn-hint"
            onClick={hint.request}
            disabled={!canHint}
            title={
              !assistReady
                ? "Download the live assist model first (sidebar)"
                : assistLoading
                  ? "The live assist model is still loading (see the bar at the top)"
                  : segments.length === 0
                  ? "Nothing to base a hint on yet"
                  : "Get a hint from the last few minutes (Ctrl/Cmd+Shift+H)"
            }
          >
            ✦ Hint
          </button>
          {segments.length > 0 && (
            <button className="btn btn-sm" onClick={copy}>
              Copy
            </button>
          )}
          {hasAudio && !transcribing && (
            <button className="btn btn-sm" onClick={run} disabled={!modelReady} title={modelReady ? undefined : "Download the speech model first"}>
              {segments.length ? "Re-transcribe" : "Transcribe"}
            </button>
          )}
        </div>
      </div>

      {transcribing && (
        <div className="row progress-row">
          <progress value={progress} max={1} aria-label="Transcription progress" />
          <span className="muted">Transcribing… {Math.round(progress * 100)}%</span>
          <button className="btn btn-sm" onClick={() => api.cancelTranscription()}>
            Cancel
          </button>
        </div>
      )}

      <HintPanel state={hint.state} onCancel={hint.cancel} onDismiss={hint.dismiss} />

      <div className="transcript-body" ref={bodyRef} onScroll={onScroll}>
        {segments.length > 0 ? (
          <ol className="segments">
            {segments.map((s, i) => (
              <li key={`${s.start_ms}-${i}`}>
                <time>{fmtDuration(s.start_ms)}</time>
                <span>
                  {labelled && <b className={`who ${s.source === "system" ? "who-others" : "who-you"}`}>{who(s)}</b>}
                  {s.text}
                </span>
              </li>
            ))}
          </ol>
        ) : (
          <p className="muted empty">
            {recording
              ? modelReady
                ? "Listening… text appears here a few seconds after you speak."
                : "Download the speech model to see live text while recording."
              : hasAudio
                ? modelReady
                  ? "No transcript yet."
                  : "Download the speech model to transcribe this recording."
                : "Record the meeting to get a transcript."}
          </p>
        )}
      </div>
      {meeting.transcript_model && segments.length > 0 && (
        <small className="muted model-note">Model: {meeting.transcript_model}</small>
      )}
    </section>
  );
}
