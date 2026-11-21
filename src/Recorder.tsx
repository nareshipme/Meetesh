import { useEffect, useRef, useState } from "react";
import { api, InputDevice, Meeting } from "./api";

const fmt = (ms: number) => {
  const s = Math.floor(ms / 1000);
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
};

interface Props {
  meeting: Meeting;
  recording: boolean;
  onChange: () => void;
  onError: (msg: string) => void;
}

/** Record / stop control for one meeting, with consent notice, timer and level meter. */
export default function Recorder({ meeting, recording, onChange, onError }: Props) {
  const [devices, setDevices] = useState<InputDevice[]>([]);
  const [device, setDevice] = useState<string>("");
  const [consent, setConsent] = useState(false);
  const [level, setLevel] = useState(0);
  const [elapsed, setElapsed] = useState(0);
  const startedAt = useRef<number>(0);

  useEffect(() => {
    api.listInputDevices().then(setDevices).catch((e) => onError(String(e)));
  }, [onError]);

  useEffect(() => {
    if (!recording) return;
    startedAt.current = Date.now();
    const timer = setInterval(() => setElapsed(Date.now() - startedAt.current), 250);
    const unlisten = api.onLevel(setLevel);
    return () => {
      clearInterval(timer);
      unlisten.then((f) => f());
      setLevel(0);
    };
  }, [recording]);

  async function start() {
    setConsent(false);
    try {
      await api.startRecording(meeting.id, device || undefined);
      onChange();
    } catch (e) {
      onError(String(e));
    }
  }

  async function stop() {
    try {
      await api.stopRecording();
    } catch (e) {
      onError(String(e));
    }
    onChange();
  }

  async function exportAudio() {
    try {
      onError(`Saved: ${await api.exportAudio(meeting.id)}`);
    } catch (e) {
      onError(String(e));
    }
  }

  if (recording) {
    return (
      <div className="recorder">
        <button className="rec" onClick={stop}>
          ■ Stop
        </button>
        <span className="timer" aria-live="off">
          {fmt(elapsed)}
        </span>
        <div className="meter" role="meter" aria-label="Microphone level" aria-valuemin={0} aria-valuemax={1} aria-valuenow={level}>
          <div className="meter-fill" style={{ width: `${Math.min(100, level * 300)}%` }} />
        </div>
      </div>
    );
  }

  const hasAudio = meeting.duration_ms > 0;
  return (
    <div className="recorder-idle">
      {meeting.status === "recovered" && (
        <p className="muted">This recording was interrupted. The audio up to that point was kept.</p>
      )}
      {hasAudio ? (
        <p className="muted">
          Recorded {fmt(meeting.duration_ms)}.{" "}
          <button onClick={exportAudio}>Export audio (WAV)</button>
        </p>
      ) : consent ? (
        <div className="consent" role="alert">
          <p>
            Make sure everyone in the meeting knows it is being recorded. Recording without consent may be illegal where
            you live.
          </p>
          <button className="rec" onClick={start}>
            ● Everyone knows, start recording
          </button>{" "}
          <button onClick={() => setConsent(false)}>Cancel</button>
        </div>
      ) : (
        <div className="row">
          <button className="primary-inline" onClick={() => setConsent(true)}>
            ● Record
          </button>
          <select aria-label="Microphone" value={device} onChange={(e) => setDevice(e.target.value)}>
            <option value="">Default microphone</option>
            {devices.map((d) => (
              <option key={d.name} value={d.name}>
                {d.name}
              </option>
            ))}
          </select>
        </div>
      )}
    </div>
  );
}
