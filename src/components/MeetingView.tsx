import { useEffect, useRef, useState } from "react";
import { api, Meeting } from "../api";
import type { ModelLoadEvent } from "../api";
import type { Notify } from "../hooks/useToasts";
import { errorText, fmtDuration, fmtWhen } from "../lib/format";
import AgendaField from "./AgendaField";
import Recorder from "./Recorder";
import SummaryPanel from "./SummaryPanel";
import Transcript from "./Transcript";

interface Props {
  meeting: Meeting;
  recording: boolean;
  modelReady: boolean;
  assistReady: boolean;
  assistLoad: ModelLoadEvent | undefined;
  notify: Notify;
  autoConsent: boolean;
  onConsentClosed: (started: boolean) => void;
  onChange: () => void;
  onBack: () => void;
  onDeleted: () => void;
}

export default function MeetingView({ meeting, recording, modelReady, assistReady, assistLoad, notify, autoConsent, onConsentClosed, onChange, onBack, onDeleted }: Props) {
  const [title, setTitle] = useState(meeting.title);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const titleRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    // A title written by the model can arrive while the user is typing their own: never overwrite that.
    if (document.activeElement !== titleRef.current) setTitle(meeting.title);
    setConfirmDelete(false);
  }, [meeting.id, meeting.title]);

  async function commitTitle() {
    const t = title.trim();
    if (!t) return setTitle(meeting.title); // never leave a meeting untitled
    if (t === meeting.title) return;
    try {
      await api.renameMeeting(meeting.id, t);
      onChange();
    } catch (e) {
      notify("error", errorText(e));
      setTitle(meeting.title);
    }
  }

  async function exportAudio() {
    try {
      notify("info", `Audio saved to ${await api.exportAudio(meeting.id)}`);
    } catch (e) {
      notify("error", errorText(e));
    }
  }

  async function remove() {
    try {
      await api.deleteMeeting(meeting.id);
      onDeleted();
    } catch (e) {
      notify("error", errorText(e));
      setConfirmDelete(false);
    }
  }

  const meta = [
    fmtWhen(meeting.created_at),
    meeting.duration_ms > 0 ? fmtDuration(meeting.duration_ms) : null,
    meeting.status === "recovered" ? "Recording was interrupted; the audio up to that point was kept" : null,
  ].filter(Boolean);

  return (
    <div className="meeting">
      <button className="back" onClick={onBack}>
        ← Meetings
      </button>

      <header className="meeting-head">
        <input
          ref={titleRef}
          className="title"
          aria-label="Meeting title"
          title={meeting.title_source === "model" ? "Written by the on-device model from the notes. Edit it to keep your own." : undefined}
          value={title}
          onChange={(e) => setTitle(e.target.value)}
          onBlur={commitTitle}
          onKeyDown={(e) => {
            if (e.key === "Enter") titleRef.current?.blur();
            if (e.key === "Escape") {
              setTitle(meeting.title);
              titleRef.current?.blur();
            }
          }}
          disabled={recording}
        />
        <p className="meta">{meta.join(" · ")}</p>
      </header>

      <div className="actions">
        <Recorder
          key={meeting.id}
          meeting={meeting}
          recording={recording}
          notify={notify}
          autoConsent={autoConsent}
          onConsentClosed={onConsentClosed}
          onChange={onChange}
        />
        <div className="row actions-end">
          {meeting.duration_ms > 0 && !recording && (
            <button className="btn btn-sm" onClick={exportAudio}>
              Export audio
            </button>
          )}
          {!recording &&
            (confirmDelete ? (
              <>
                <span className="muted">Delete this meeting and its audio?</span>
                <button className="btn btn-sm btn-danger" onClick={remove}>
                  Delete
                </button>
                <button className="btn btn-sm" onClick={() => setConfirmDelete(false)}>
                  Cancel
                </button>
              </>
            ) : (
              <button className="btn btn-sm btn-ghost-danger" onClick={() => setConfirmDelete(true)}>
                Delete
              </button>
            ))}
        </div>
      </div>

      <AgendaField meeting={meeting} notify={notify} onChange={onChange} />

      <SummaryPanel meeting={meeting} recording={recording} assistReady={assistReady} notify={notify} onChange={onChange} />

      <Transcript
        meeting={meeting}
        modelReady={modelReady}
        assistReady={assistReady}
        assistLoad={assistLoad}
        recording={recording}
        notify={notify}
        onChange={onChange}
      />
    </div>
  );
}
