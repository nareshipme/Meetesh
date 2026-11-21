import { useCallback, useEffect, useRef, useState } from "react";
import { api, Meeting, TranscriptSegment } from "./api";

const stamp = (ms: number) => {
  const s = Math.floor(ms / 1000);
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
};

interface Props {
  meeting: Meeting;
  modelReady: boolean;
  recording: boolean;
  onError: (msg: string) => void;
  onChange: () => void;
}

export default function Transcript({ meeting, modelReady, recording, onError, onChange }: Props) {
  const [segments, setSegments] = useState<TranscriptSegment[]>([]);
  const [progress, setProgress] = useState<number | null>(null);

  const load = useCallback(() => {
    api.getTranscript(meeting.id).then(setSegments).catch((e) => onError(String(e)));
  }, [meeting.id, onError]);

  useEffect(() => {
    load();
    api.activeTranscription().then((id) => setProgress(id === meeting.id ? 0 : null));
    const un = api.onTranscribe((e) => {
      if (e.meeting_id !== meeting.id) return;
      if (e.done) {
        setProgress(null);
        if (e.error) onError(e.error);
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
      load(); // pick up any gap-filling pass result
      onChange();
    });
    return () => {
      un.then((f) => f());
      unLive.then((f) => f());
      unDone.then((f) => f());
    };
  }, [meeting.id, load, onError, onChange]);

  async function run() {
    try {
      setProgress(0);
      await api.transcribeMeeting(meeting.id);
    } catch (e) {
      setProgress(null);
      onError(String(e));
    }
  }

  const hasAudio = meeting.duration_ms > 0 && !recording;
  const listRef = useRef<HTMLOListElement>(null);
  useEffect(() => {
    if (recording) listRef.current?.lastElementChild?.scrollIntoView({ block: "nearest" });
  }, [segments, recording]);

  return (
    <section>
      <h2>
        Transcript {recording && modelReady && <span className="live-badge">● Live</span>}
      </h2>
      {recording && !modelReady && (
        <p className="muted">Download the speech model to see live text while recording.</p>
      )}
      {progress !== null && (
        <div className="row">
          <progress value={progress} max={1} aria-label="Transcription progress" />
          <span className="muted">Transcribing… {Math.round(progress * 100)}%</span>
          <button onClick={() => api.cancelTranscription()}>Cancel</button>
        </div>
      )}
      {progress === null && hasAudio && (
        <div className="row">
          <button onClick={run} disabled={!modelReady}>
            {segments.length ? "Re-transcribe" : "Transcribe"}
          </button>
          {!modelReady && <span className="muted">Download the speech model first.</span>}
          {meeting.transcript_model && <span className="muted">Model: {meeting.transcript_model}</span>}
        </div>
      )}
      {segments.length > 0 ? (
        <ol className="segments" ref={listRef}>
          {segments.map((s, i) => (
            <li key={i}>
              <time>{stamp(s.start_ms)}</time> {s.text}
            </li>
          ))}
        </ol>
      ) : (
        progress === null &&
        !recording && <p className="muted">{hasAudio ? "No transcript yet." : "Record to get a transcript."}</p>
      )}
    </section>
  );
}
