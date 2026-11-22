import { useEffect, useState } from "react";
import { api, Meeting, SummaryStatus } from "../api";
import type { Notify } from "../hooks/useToasts";
import { errorText } from "../lib/format";

interface Props {
  meeting: Meeting;
  assistReady: boolean;
  notify: Notify;
  onChange: () => void;
  onOpenSettings: () => void;
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

/** Where the notes were written: on this device, or by an online model (the text was sent there). */
function writtenBy(model: string): string {
  if (!model) return "";
  if (model.startsWith("online:")) return `Written online by ${model.slice(7)} (the transcript text was sent to that provider)`;
  return `Written on this device by ${model}`;
}

/** Meeting notes (summary, decisions, action items), written after a meeting by the on-device model or, if chosen, an online one. */
export default function SummaryPanel({ meeting, assistReady, notify, onChange, onOpenSettings }: Props) {
  // null: nothing is being written for this meeting. Otherwise where the notes are in the background job.
  const [progress, setProgress] = useState<(SummaryStatus & { done: number; total: number }) | null>(null);

  useEffect(() => {
    setProgress(null);
    api
      .summaryStatus(meeting.id)
      .then((s) => s.state !== "idle" && setProgress({ ...s, done: 0, total: 1 }))
      .catch(() => {});
    const un = api.onSummary((e) => {
      if (e.meeting_id !== meeting.id) return;
      if (e.state === "finished") {
        setProgress(null);
        if (e.error) notify("error", e.error);
        onChange();
        return;
      }
      setProgress({ state: e.state, position: e.position, paused_for: e.paused_for, done: e.done, total: e.total });
    });
    return () => {
      un.then((f) => f());
    };
  }, [meeting.id, notify, onChange]);

  const hasTranscript = meeting.transcript_model !== "";

  async function run() {
    setProgress({ state: "queued", position: 0, paused_for: null, done: 0, total: 1 });
    try {
      await api.summarizeMeeting(meeting.id); // joins the line; progress comes as events
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

  if (progress) {
    const waiting = progress.state === "queued";
    const paused = progress.state === "paused";
    return (
      <section className="notes">
        <div className="row progress-row">
          {waiting ? <progress aria-label="Waiting for its turn" /> : <progress value={progress.done} max={Math.max(progress.total, 1)} aria-label="Writing notes" />}
          <span className="muted">
            {waiting
              ? progress.position > 0
                ? `Waiting for its turn: ${progress.position} ahead of this one. It starts by itself.`
                : "Starting…"
              : paused
                ? progress.paused_for === "recording"
                  ? "Paused while you record, so live transcription keeps up. It carries on by itself afterwards."
                  : "Paused while a hint is written. It carries on by itself."
                : `Writing notes… ${progress.total > 1 ? `step ${Math.min(progress.done + 1, progress.total)} of ${progress.total}` : ""}`}
          </span>
          <button className="btn btn-sm" onClick={() => api.cancelSummary(meeting.id)}>
            Cancel
          </button>
        </div>
        {meeting.summary && <Notes text={meeting.summary} />}
      </section>
    );
  }

  if (!meeting.summary) {
    return (
      <section className="notes notes-empty">
        <p className="muted">
          {!hasTranscript
            ? "There is no transcript yet, so there are no notes."
            : assistReady
              ? "No notes yet. The on-device model writes a summary, decisions and action items."
              : "Notes are written by the on-device model."}
        </p>
        {hasTranscript &&
          (assistReady ? (
            <button className="btn btn-hint" onClick={run}>
              ✦ Write meeting notes
            </button>
          ) : (
            <button className="btn" onClick={onOpenSettings}>
              Set up the model in Settings
            </button>
          ))}
      </section>
    );
  }

  return (
    <section className="notes">
      <div className="notes-bar">
        <small className="muted">{writtenBy(meeting.summary_model)}</small>
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
