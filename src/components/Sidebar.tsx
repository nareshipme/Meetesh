import type { Meeting } from "../api";
import type { ModelsState } from "../hooks/useModels";
import { fmtDuration, fmtWhen } from "../lib/format";
import ModelPanel from "./ModelPanel";

interface Props {
  meetings: Meeting[];
  selectedId: number | null;
  recordingId: number | null;
  transcribingId: number | null;
  models: ModelsState;
  onSelect: (id: number) => void;
  onNew: () => void;
  hidden: boolean; // on narrow screens only one pane shows at a time
}

function statusChip(m: Meeting, recordingId: number | null, transcribingId: number | null) {
  if (m.id === recordingId) return <span className="chip chip-rec">● Recording</span>;
  if (m.id === transcribingId) return <span className="chip chip-busy">Transcribing…</span>;
  if (m.status === "recovered") return <span className="chip chip-warn">Interrupted</span>;
  return null;
}

export default function Sidebar({ meetings, selectedId, recordingId, transcribingId, models, onSelect, onNew, hidden }: Props) {
  return (
    <aside className={hidden ? "sidebar hide-on-mobile" : "sidebar"}>
      <div className="sidebar-top">
        <h1 className="brand">Meetesh</h1>
        <button className="btn btn-primary btn-block" onClick={onNew}>
          + New meeting
        </button>
      </div>

      <nav className="meeting-list" aria-label="Meetings">
        {meetings.length === 0 && <p className="muted pad">No meetings yet.</p>}
        {meetings.map((m) => (
          <button
            key={m.id}
            className={m.id === selectedId ? "item active" : "item"}
            aria-current={m.id === selectedId ? "true" : undefined}
            onClick={() => onSelect(m.id)}
          >
            <span className="item-title">{m.title}</span>
            <span className="item-meta">
              {fmtWhen(m.created_at)}
              {m.duration_ms > 0 && <> · {fmtDuration(m.duration_ms)}</>}
              {statusChip(m, recordingId, transcribingId)}
            </span>
          </button>
        ))}
      </nav>

      <ModelPanel ms={models} />
    </aside>
  );
}
