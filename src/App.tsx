import { useCallback, useEffect, useState } from "react";
import { api } from "./api";
import MeetingView from "./components/MeetingView";
import LoadingBanner from "./components/LoadingBanner";
import Sidebar from "./components/Sidebar";
import Toasts from "./components/Toasts";
import { useMeetings } from "./hooks/useMeetings";
import { useModels } from "./hooks/useModels";
import { useToasts } from "./hooks/useToasts";
import { errorText } from "./lib/format";

export default function App() {
  const { toasts, notify, dismiss } = useToasts();
  const { meetings, recordingId, transcribingId, refresh } = useMeetings(notify);
  const models = useModels(notify);
  const [selectedId, setSelectedId] = useState<number | null>(null);
  // A meeting just created by "New recording": its consent notice opens straight away.
  const [pendingConsent, setPendingConsent] = useState<{ id: number; title: string } | null>(null);

  const selected = meetings.find((m) => m.id === selectedId) ?? null;

  /** `recording`: also open the consent notice, so one confirmation starts recording. */
  const createMeeting = useCallback(
    async (recording: boolean) => {
      if (recording && recordingId !== null) {
        // Only one recording at a time: go to the one in progress.
        setSelectedId(recordingId);
        notify("info", "A recording is already in progress");
        return;
      }
      try {
        const m = await api.createMeeting();
        await refresh();
        setSelectedId(m.id);
        setPendingConsent(recording ? { id: m.id, title: m.title } : null);
      } catch (e) {
        notify("error", errorText(e));
      }
    },
    [refresh, notify, recordingId],
  );
  const newRecording = useCallback(() => createMeeting(true), [createMeeting]);
  const newNotes = useCallback(() => createMeeting(false), [createMeeting]);

  // Cmd/Ctrl+N starts a new recording.
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if ((e.metaKey || e.ctrlKey) && !e.shiftKey && e.key.toLowerCase() === "n") {
        e.preventDefault();
        newRecording();
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [newRecording]);

  /** The consent notice of an auto-opened meeting was closed. If it was cancelled and nothing was
   *  entered, remove the empty meeting so cancelling leaves no clutter behind. */
  const onConsentClosed = useCallback(
    async (started: boolean) => {
      const p = pendingConsent;
      setPendingConsent(null);
      if (!p || started) return;
      const m = meetings.find((x) => x.id === p.id);
      if (m && m.title === p.title && m.agenda === "" && m.duration_ms === 0 && m.status === "stopped") {
        try {
          await api.deleteMeeting(p.id);
          setSelectedId(null);
          refresh();
        } catch {
          /* leaving an empty meeting behind is harmless */
        }
      }
    },
    [pendingConsent, meetings, refresh],
  );

  return (
    <div className="app">
      <Sidebar
        meetings={meetings}
        selectedId={selectedId}
        recordingId={recordingId}
        transcribingId={transcribingId}
        models={models}
        onSelect={setSelectedId}
        onNewRecording={newRecording}
        onNewNotes={newNotes}
        hidden={selected !== null}
      />
      <main className={selected ? "main" : "main hide-on-mobile"}>
        <LoadingBanner ms={models} />
        {selected ? (
          <MeetingView
            meeting={selected}
            recording={recordingId === selected.id}
            modelReady={models.speechInstalled}
            assistReady={models.assistInstalled}
            assistLoad={models.assistLoad}
            notify={notify}
            autoConsent={pendingConsent?.id === selected.id}
            onConsentClosed={onConsentClosed}
            onChange={refresh}
            onBack={() => setSelectedId(null)}
            onDeleted={() => {
              setSelectedId(null);
              refresh();
            }}
          />
        ) : (
          <div className="welcome">
            <h2>Welcome to Meetesh</h2>
            <p className="muted">Private meeting notes. Everything runs on this device.</p>
            <ol>
              {!models.speechInstalled && <li>Download the speech model (once) from the panel at the bottom left.</li>}
              <li>Press New recording and confirm.</li>
              <li>Talk. Live text appears as you speak.</li>
            </ol>
            <div className="row">
              <button className="btn btn-primary" onClick={newRecording}>
                ● New recording
              </button>
              <button className="btn" onClick={newNotes}>
                Notes only
              </button>
            </div>
            <small className="muted">Shortcut: Ctrl/Cmd+N</small>
          </div>
        )}
      </main>
      <Toasts toasts={toasts} onDismiss={dismiss} />
    </div>
  );
}
