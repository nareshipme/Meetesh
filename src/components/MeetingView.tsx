import { useCallback, useEffect, useRef, useState } from "react";
import { api, Meeting } from "../api";
import type { ModelLoadEvent } from "../api";
import { useAutoHints } from "../hooks/useAutoHints";
import { useHint } from "../hooks/useHint";
import { useLivePoints } from "../hooks/useLivePoints";
import type { Notify } from "../hooks/useToasts";
import { useTranscript } from "../hooks/useTranscript";
import { errorText, fmtDuration, fmtWhen } from "../lib/format";
import AgendaField from "./AgendaField";
import LiveAssist from "./LiveAssist";
import Menu, { type MenuItem } from "./Menu";
import Recorder from "./Recorder";
import SummaryPanel from "./SummaryPanel";
import Tabs from "./Tabs";
import TranscriptList from "./TranscriptList";

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
  onOpenSettings: () => void;
  /** Open another meeting (used by hints that point at an earlier one). */
  onOpenMeeting: (id: number) => void;
}

type TabId = "notes" | "transcript" | "goals";

export default function MeetingView(props: Props) {
  const { meeting, recording, notify, onChange, onBack, onDeleted } = props;
  const [title, setTitle] = useState(meeting.title);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const titleRef = useRef<HTMLInputElement>(null);
  const transcript = useTranscript(meeting, notify, onChange);

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

  const hasAudio = meeting.duration_ms > 0;
  const labelled = meeting.has_system_audio || transcript.segments.some((s) => s.source === "system");

  const copyText = useCallback(async (text: string, done: string) => {
    try {
      await navigator.clipboard.writeText(text);
      notify("info", done);
    } catch {
      notify("error", "Could not copy to the clipboard");
    }
  }, [notify]);

  const items: MenuItem[] = [
    {
      label: "Copy as Markdown",
      onSelect: async () => {
        try {
          copyText(await api.meetingMarkdown(meeting.id), "Meeting copied as Markdown");
        } catch (e) {
          notify("error", errorText(e));
        }
      },
    },
    {
      label: "Export Markdown file",
      onSelect: async () => {
        try {
          notify("info", `Markdown saved to ${await api.exportMarkdown(meeting.id)}`);
        } catch (e) {
          notify("error", errorText(e));
        }
      },
    },
    ...(transcript.segments.length > 0
      ? [{ label: "Copy transcript", onSelect: () => copyText(transcript.segments.map((s) => `${fmtDuration(s.start_ms)}  ${labelled ? (s.source === "system" ? "Others: " : "You: ") : ""}${s.text}`).join("\n"), "Transcript copied") }]
      : []),
    ...(hasAudio
      ? [
          {
            label: "Export audio (WAV)",
            onSelect: async () => {
              try {
                notify("info", `Audio saved to ${await api.exportAudio(meeting.id)}`);
              } catch (e) {
                notify("error", errorText(e));
              }
            },
          },
          {
            label: transcript.segments.length ? "Re-transcribe" : "Transcribe",
            disabled: !props.modelReady || transcript.transcribing,
            onSelect: transcript.run,
          },
        ]
      : []),
    { label: "Delete meeting…", danger: true, separated: true, onSelect: () => setConfirmDelete(true) },
  ];

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
    hasAudio ? fmtDuration(meeting.duration_ms) : null,
    meeting.status === "recovered" ? "Recording was interrupted; the audio up to that point was kept" : null,
  ].filter(Boolean);

  return (
    <div className="meeting">
      <button className="back" onClick={onBack}>
        ← Meetings
      </button>

      <header className="meeting-head">
        <div className="title-row">
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
          {!recording && <Menu items={items} label="More actions" />}
        </div>
        <p className="meta">{meta.join(" · ")}</p>
        {confirmDelete && (
          <div className="confirm-bar" role="alert">
            <span>Delete this meeting and its audio?</span>
            <button className="btn btn-sm btn-danger" onClick={remove}>
              Delete
            </button>
            <button className="btn btn-sm" onClick={() => setConfirmDelete(false)}>
              Cancel
            </button>
          </div>
        )}
      </header>

      {recording ? <LiveView {...props} transcript={transcript} labelled={labelled} /> : <ReviewView {...props} transcript={transcript} labelled={labelled} hasAudio={hasAudio} />}
    </div>
  );
}

type TranscriptState = ReturnType<typeof useTranscript>;

/** While recording: the transcript fills the left, hints and goals sit in a column on the right. */
function LiveView({ meeting, modelReady, assistReady, assistLoad, notify, onChange, onOpenSettings, onOpenMeeting, autoConsent, onConsentClosed, transcript, labelled }: Props & { transcript: TranscriptState; labelled: boolean }) {
  const hint = useHint(meeting.id, notify);
  const auto = useAutoHints(notify);
  const points = useLivePoints(meeting.id, notify);
  const assistLoading = assistLoad?.state === "loading" || assistLoad?.state === "warming";
  const blocked = !assistReady
    ? "Download a live assist model first (Settings)"
    : assistLoading
      ? "The live assist model is still loading (see the bar at the top)"
      : transcript.segments.length === 0
        ? "Nothing to base a hint on yet"
        : hint.state.kind === "working"
          ? "A hint is already being written"
          : "";

  // Cmd/Ctrl+Shift+H asks for a hint (in-app shortcut; a global hotkey comes later).
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if ((e.metaKey || e.ctrlKey) && e.shiftKey && e.key.toLowerCase() === "h" && blocked === "") {
        e.preventDefault();
        hint.request();
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [blocked, hint.request]);

  return (
    <div className="live">
      <div className="live-main">
        <Recorder key={meeting.id} meeting={meeting} recording notify={notify} autoConsent={autoConsent} onConsentClosed={onConsentClosed} onChange={onChange} />
        <div className="live-label">
          <h2>
            Transcript <span className="live-badge">{modelReady ? "Live" : ""}</span>
          </h2>
        </div>
        <TranscriptList
          segments={transcript.segments}
          following
          labelled={labelled}
          empty={modelReady ? "Listening… text appears here a few seconds after you speak." : "Download the speech model (Settings) to see live text while recording."}
        />
      </div>
      <LiveAssist meeting={meeting} hint={hint} auto={auto} points={points} assistReady={assistReady} hintBlockedReason={blocked} notify={notify} onChange={onChange} onOpenSettings={onOpenSettings} onOpenMeeting={onOpenMeeting} />
    </div>
  );
}

/** After (or before) recording: tabs for the notes, the transcript and the goals. */
function ReviewView({ meeting, modelReady, assistReady, notify, onChange, onOpenSettings, autoConsent, onConsentClosed, transcript, labelled, hasAudio }: Props & { transcript: TranscriptState; labelled: boolean; hasAudio: boolean }) {
  const [tab, setTab] = useState<TabId>(meeting.summary ? "notes" : "transcript");
  const [writing, setWriting] = useState(false); // notes are being written
  const picked = useRef(false); // the user chose a tab themselves: stop switching for them
  useEffect(() => {
    picked.current = false;
    setTab(meeting.summary ? "notes" : "transcript");
    setWriting(false);
  }, [meeting.id]); // eslint-disable-line react-hooks/exhaustive-deps

  // Notes start by themselves after a recording: show them, unless the user is already somewhere on purpose.
  useEffect(() => {
    const un = api.onSummary((e) => {
      if (e.meeting_id !== meeting.id) return;
      setWriting(!e.finished);
      if (!e.finished && !picked.current) setTab("notes");
    });
    return () => {
      un.then((f) => f());
    };
  }, [meeting.id]);

  // A meeting with no recording yet: record, or write goals for later.
  if (!hasAudio) {
    return (
      <div className="review-empty">
        <Recorder key={meeting.id} meeting={meeting} recording={false} notify={notify} autoConsent={autoConsent} onConsentClosed={onConsentClosed} onChange={onChange} />
        <AgendaField plain meeting={meeting} notify={notify} onChange={onChange} />
        <p className="muted">Record the meeting to get a transcript and notes.</p>
      </div>
    );
  }

  return (
    <div className="review">
      <Tabs
        label="Meeting sections"
        active={tab}
        onChange={(t) => {
          picked.current = true;
          setTab(t as TabId);
        }}
        tabs={[
          { id: "notes", label: "Notes", badge: writing ? <span className="tab-dot busy" aria-label="being written" /> : undefined },
          { id: "transcript", label: "Transcript", badge: transcript.segments.length ? <span className="tab-count">{transcript.segments.length}</span> : undefined },
          { id: "goals", label: "Goals", badge: meeting.agenda ? <span className="tab-dot" aria-label="has text" /> : undefined },
        ]}
      />
      <div className="tab-panel" role="tabpanel" id={`panel-${tab}`} aria-labelledby={`tab-${tab}`}>
        {tab === "notes" && <SummaryPanel meeting={meeting} assistReady={assistReady} notify={notify} onChange={onChange} onOpenSettings={onOpenSettings} />}
        {tab === "transcript" && (
          <>
            {transcript.transcribing && (
              <div className="row progress-row">
                <progress value={transcript.progress ?? 0} max={1} aria-label="Transcription progress" />
                <span className="muted">Transcribing… {Math.round((transcript.progress ?? 0) * 100)}%</span>
                <button className="btn btn-sm" onClick={() => api.cancelTranscription()}>
                  Cancel
                </button>
              </div>
            )}
            <TranscriptList
              segments={transcript.segments}
              following={transcript.transcribing}
              labelled={labelled}
              empty={
                modelReady ? (
                  <>
                    No transcript yet.{" "}
                    <button className="link-btn inline" onClick={transcript.run} disabled={transcript.transcribing}>
                      Transcribe this recording
                    </button>
                  </>
                ) : (
                  <>
                    Download a speech model to transcribe this recording.{" "}
                    <button className="link-btn inline" onClick={onOpenSettings}>
                      Open Settings
                    </button>
                  </>
                )
              }
            />
            {meeting.transcript_model && transcript.segments.length > 0 && <small className="muted model-note">Model: {meeting.transcript_model}</small>}
          </>
        )}
        {tab === "goals" && <AgendaField plain meeting={meeting} notify={notify} onChange={onChange} />}
      </div>
    </div>
  );
}
