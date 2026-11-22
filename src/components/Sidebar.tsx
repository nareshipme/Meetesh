import { useEffect, useRef, useState } from "react";
import type { Meeting } from "../api";
import { useSearch } from "../hooks/useSearch";
import SearchResults from "./SearchResults";
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
  onNewRecording: () => void;
  onNewNotes: () => void;
  hidden: boolean; // on narrow screens only one pane shows at a time
}

function statusChip(m: Meeting, recordingId: number | null, transcribingId: number | null) {
  if (m.id === recordingId) return <span className="chip chip-rec">● Recording</span>;
  if (m.id === transcribingId) return <span className="chip chip-busy">Transcribing…</span>;
  if (m.status === "recovered") return <span className="chip chip-warn">Interrupted</span>;
  return null;
}

export default function Sidebar({ meetings, selectedId, recordingId, transcribingId, models, onSelect, onNewRecording, onNewNotes, hidden }: Props) {
  const [query, setQuery] = useState("");
  const search = useSearch(query);
  const searchRef = useRef<HTMLInputElement>(null);

  // Ctrl/Cmd+K jumps to the search box.
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        searchRef.current?.focus();
        searchRef.current?.select();
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  return (
    <aside className={hidden ? "sidebar hide-on-mobile" : "sidebar"}>
      <div className="sidebar-top">
        <h1 className="brand">Meetesh</h1>
        <button className="btn btn-primary btn-block" onClick={onNewRecording} title="Start a new recording (Ctrl/Cmd+N)">
          ● New recording
        </button>
        <button className="link-btn" onClick={onNewNotes}>
          or a meeting for notes only
        </button>
      </div>

      <div className="search">
        <input
          ref={searchRef}
          type="search"
          placeholder="Search meetings (Ctrl/Cmd+K)"
          aria-label="Search meetings"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Escape") setQuery("");
          }}
        />
      </div>

      <nav className="meeting-list" aria-label={query.trim() ? "Search results" : "Meetings"}>
        {query.trim() !== "" && (
          <SearchResults query={query} results={search.results} error={search.error} selectedId={selectedId} onSelect={onSelect} />
        )}
        {query.trim() === "" && meetings.length === 0 && <p className="muted pad">No meetings yet.</p>}
        {query.trim() === "" && meetings.map((m) => (
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
