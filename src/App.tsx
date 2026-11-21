import { useCallback, useEffect, useState } from "react";
import { api, Meeting } from "./api";
import ModelPanel from "./ModelPanel";
import Recorder from "./Recorder";
import Transcript from "./Transcript";

export default function App() {
  const [meetings, setMeetings] = useState<Meeting[]>([]);
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [recordingId, setRecordingId] = useState<number | null>(null);
  const [modelReady, setModelReady] = useState(false);

  const refresh = useCallback(async () => {
    try {
      setMeetings(await api.listMeetings());
      setRecordingId(await api.activeRecording());
      setModelReady((await api.listModels()).some((m) => m.is_default && m.installed));
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const selected = meetings.find((m) => m.id === selectedId) ?? null;

  async function newMeeting() {
    try {
      const m = await api.createMeeting();
      await refresh();
      setSelectedId(m.id);
    } catch (e) {
      setError(String(e));
    }
  }

  async function rename(m: Meeting, title: string) {
    if (title.trim() && title !== m.title) {
      await api.renameMeeting(m.id, title.trim()).catch((e) => setError(String(e)));
      refresh();
    }
  }

  async function remove(m: Meeting) {
    if (!window.confirm(`Delete "${m.title}"?`)) return;
    await api.deleteMeeting(m.id).catch((e) => setError(String(e)));
    setSelectedId(null);
    refresh();
  }

  return (
    <div className="app">
      <aside className={selected ? "sidebar hide-on-mobile" : "sidebar"}>
        <h1>Meetesh</h1>
        <button className="primary" onClick={newMeeting}>
          + New meeting
        </button>
        <ul className="list">
          {meetings.map((m) => (
            <li key={m.id}>
              <button
                className={m.id === selectedId ? "item active" : "item"}
                onClick={() => setSelectedId(m.id)}
              >
                {m.id === recordingId ? "● " : ""}
                {m.title}
                <small>{new Date(m.created_at * 1000).toLocaleString()}</small>
              </button>
            </li>
          ))}
        </ul>
        {meetings.length === 0 && <p className="muted">No meetings yet.</p>}
        <ModelPanel onChange={refresh} />
      </aside>

      <main className={selected ? "main" : "main hide-on-mobile"}>
        {error && (
          <p className="error" role="alert">
            {error} <button onClick={() => setError(null)}>dismiss</button>
          </p>
        )}
        {selected ? (
          <>
            <button className="back" onClick={() => setSelectedId(null)}>
              ← Meetings
            </button>
            <input
              key={selected.id}
              className="title"
              aria-label="Meeting title"
              defaultValue={selected.title}
              onBlur={(e) => rename(selected, e.target.value)}
            />
            <Recorder
              meeting={selected}
              recording={recordingId === selected.id}
              onChange={refresh}
              onError={setError}
            />
            <Transcript
              meeting={selected}
              modelReady={modelReady}
              recording={recordingId === selected.id}
              onError={setError}
              onChange={refresh}
            />
            <button onClick={() => remove(selected)} disabled={recordingId === selected.id}>
              Delete
            </button>
          </>
        ) : (
          <p className="muted">Select a meeting or create a new one.</p>
        )}
      </main>
    </div>
  );
}
