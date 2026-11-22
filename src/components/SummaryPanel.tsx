import { useEffect, useState } from "react";
import { api, Meeting } from "../api";
import type { Notify } from "../hooks/useToasts";
import { errorText } from "../lib/format";

interface Props {
  meeting: Meeting;
  recording: boolean;
  assistReady: boolean;
  notify: Notify;
  onChange: () => void;
}

/** The notes are stored as small Markdown: "## Heading", "- item" and plain paragraphs. */
function Notes({ text }: { text: string }) {
  const blocks: JSX.Element[] = [];
  let items: string[] = [];
  const flush = () => {
    if (items.length) blocks.push(<ul key={`l${blocks.length}`}>{items.map((t, i) => <li key={i}>{t}</li>)}</ul>);
    items = [];
  };
  for (const line of text.split("\n")) {
    if (line.startsWith("- ")) items.push(line.slice(2));
    else {
      flush();
      if (line.startsWith("## ")) blocks.push(<h3 key={`h${blocks.length}`}>{line.slice(3)}</h3>);
      else if (line.trim()) blocks.push(<p key={`p${blocks.length}`}>{line}</p>);
    }
  }
  flush();
  return <div className="notes-body">{blocks}</div>;
}

/** Meeting notes: summary, decisions and action items written by the on-device model after a meeting. */
export default function SummaryPanel({ meeting, recording, assistReady, notify, onChange }: Props) {
  const [progress, setProgress] = useState<{ done: number; total: number } | null>(null);

  useEffect(() => {
    setProgress(null);
    api.activeSummary().then((id) => id === meeting.id && setProgress({ done: 0, total: 1 })).catch(() => {});
    const un = api.onSummary((e) => {
      if (e.meeting_id !== meeting.id) return;
      if (!e.finished) return setProgress({ done: e.done, total: e.total });
      setProgress(null);
      if (e.error) notify("error", e.error);
      onChange();
    });
    return () => {
      un.then((f) => f());
    };
  }, [meeting.id, notify, onChange]);

  const hasTranscript = meeting.transcript_model !== "";
  if (recording || (!meeting.summary && !progress && !hasTranscript)) return null;

  async function run() {
    setProgress({ done: 0, total: 1 });
    try {
      await api.summarizeMeeting(meeting.id);
    } catch (e) {
      setProgress(null);
      notify("error", errorText(e));
    }
  }

  async function remove() {
    try {
      await api.clearSummary(meeting.id);
      onChange();
    } catch (e) {
      notify("error", errorText(e));
    }
  }

  async function copy() {
    try {
      await navigator.clipboard.writeText(meeting.summary);
      notify("info", "Notes copied");
    } catch {
      notify("error", "Could not copy to the clipboard");
    }
  }

  const title = (
    <h2>
      Notes
      {meeting.summary_model && !progress && <span className="muted note-model"> · written on this device by {meeting.summary_model}</span>}
    </h2>
  );

  if (progress) {
    return (
      <section className="notes">
        <div className="transcript-head">{title}</div>
        <div className="row progress-row">
          <progress value={progress.done} max={Math.max(progress.total, 1)} aria-label="Writing notes" />
          <span className="muted">
            Writing notes… {progress.total > 1 ? `step ${Math.min(progress.done + 1, progress.total)} of ${progress.total}` : ""}
          </span>
          <button className="btn btn-sm" onClick={() => api.cancelSummary()}>
            Cancel
          </button>
        </div>
      </section>
    );
  }

  if (!meeting.summary) {
    return (
      <section className="notes">
        <div className="transcript-head">
          {title}
          <button
            className="btn btn-sm btn-hint"
            onClick={run}
            disabled={!assistReady}
            title={assistReady ? "Summary, decisions and action items from the transcript" : "Download the live assist model first (sidebar); it also writes the notes"}
          >
            ✦ Write meeting notes
          </button>
        </div>
      </section>
    );
  }

  return (
    <section className="notes">
      <div className="transcript-head">
        {title}
        <div className="row">
          <button className="btn btn-sm" onClick={copy}>
            Copy
          </button>
          <button className="btn btn-sm" onClick={run} disabled={!assistReady} title="Write the notes again from the current transcript">
            Regenerate
          </button>
          <button className="btn btn-sm btn-ghost-danger" onClick={remove}>
            Remove
          </button>
        </div>
      </div>
      <Notes text={meeting.summary} />
    </section>
  );
}
