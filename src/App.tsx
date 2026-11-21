import { useCallback, useState } from "react";
import { api } from "./api";
import MeetingView from "./components/MeetingView";
import Sidebar from "./components/Sidebar";
import Toasts from "./components/Toasts";
import { useMeetings } from "./hooks/useMeetings";
import { useToasts } from "./hooks/useToasts";
import { errorText } from "./lib/format";

export default function App() {
  const { toasts, notify, dismiss } = useToasts();
  const { meetings, recordingId, transcribingId, modelReady, assistReady, refresh } = useMeetings(notify);
  const [selectedId, setSelectedId] = useState<number | null>(null);

  const selected = meetings.find((m) => m.id === selectedId) ?? null;

  const newMeeting = useCallback(async () => {
    try {
      const m = await api.createMeeting();
      await refresh();
      setSelectedId(m.id);
    } catch (e) {
      notify("error", errorText(e));
    }
  }, [refresh, notify]);

  return (
    <div className="app">
      <Sidebar
        meetings={meetings}
        selectedId={selectedId}
        recordingId={recordingId}
        transcribingId={transcribingId}
        notify={notify}
        onSelect={setSelectedId}
        onNew={newMeeting}
        onRefresh={refresh}
        hidden={selected !== null}
      />
      <main className={selected ? "main" : "main hide-on-mobile"}>
        {selected ? (
          <MeetingView
            meeting={selected}
            recording={recordingId === selected.id}
            modelReady={modelReady}
            assistReady={assistReady}
            notify={notify}
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
              {!modelReady && <li>Download the speech model (once) from the panel at the bottom left.</li>}
              <li>Create a meeting and press Record.</li>
              <li>Talk. Live text appears as you speak.</li>
            </ol>
            <button className="btn btn-primary" onClick={newMeeting}>
              + New meeting
            </button>
          </div>
        )}
      </main>
      <Toasts toasts={toasts} onDismiss={dismiss} />
    </div>
  );
}
