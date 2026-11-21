import { useEffect, useRef, useState } from "react";
import { api, Meeting } from "../api";
import type { Notify } from "../hooks/useToasts";
import { errorText, fmtDuration, fmtWhen } from "../lib/format";
import Recorder from "./Recorder";
import Transcript from "./Transcript";

interface Props {
  meeting: Meeting;
  recording: boolean;
  modelReady: boolean;
  notify: Notify;
  onChange: () => void;
  onBack: () => void;
  onDeleted: () => void;
}

export default function MeetingView({ meeting, recording, modelReady, notify, onChange, onBack, onDeleted }: Props) {
  const [title, setTitle] = useState(meeting.title);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const titleRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    setTitle(meeting.title);
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
        <Recorder meeting={meeting} recording={recording} notify={notify} onChange={onChange} />
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

      <Transcript meeting={meeting} modelReady={modelReady} recording={recording} notify={notify} onChange={onChange} />
    </div>
  );
}
