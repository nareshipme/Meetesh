import { useEffect, useRef, useState } from "react";
import { api, InputDevice, Meeting } from "../api";
import type { Notify } from "../hooks/useToasts";
import { errorText, fmtDuration } from "../lib/format";
import LevelMeter from "./LevelMeter";

interface Props {
  meeting: Meeting;
  recording: boolean;
  notify: Notify;
  onChange: () => void;
}

/** Record / stop controls for one meeting: consent notice, microphone choice, timer, level meter. */
export default function Recorder({ meeting, recording, notify, onChange }: Props) {
  const [devices, setDevices] = useState<InputDevice[]>([]);
  const [device, setDevice] = useState("");
  const [consent, setConsent] = useState(false);
  const [level, setLevel] = useState(0);
  const [elapsed, setElapsed] = useState(0);
  const [busy, setBusy] = useState(false);
  const startedAt = useRef(0);

  useEffect(() => {
    api.listInputDevices().then(setDevices).catch((e) => notify("error", errorText(e)));
  }, [notify]);

  useEffect(() => {
    if (!recording) return;
    startedAt.current = Date.now();
    setElapsed(0);
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
    setBusy(true);
    try {
      await api.startRecording(meeting.id, device || undefined);
    } catch (e) {
      notify("error", errorText(e));
    }
    setBusy(false);
    onChange();
  }

  async function stop() {
    setBusy(true);
    try {
      await api.stopRecording();
    } catch (e) {
      notify("error", errorText(e));
    }
    setBusy(false);
    onChange();
  }

  if (recording) {
    return (
      <div className="recorder recording">
        <button className="btn btn-danger" onClick={stop} disabled={busy}>
          ■ Stop
        </button>
        <span className="timer" aria-label="Elapsed time">
          {fmtDuration(elapsed)}
        </span>
        <LevelMeter level={level} />
      </div>
    );
  }

  // One recording per meeting for now: once it has audio, recording controls give way to its status.
  if (meeting.duration_ms > 0) return null;

  if (consent) {
    return (
      <div className="consent" role="alert">
        <p>
          Make sure everyone in the meeting knows it is being recorded. Recording without consent may be illegal where you
          live.
        </p>
        <div className="row">
          <button className="btn btn-danger" onClick={start} disabled={busy}>
            ● Everyone knows, start recording
          </button>
          <button className="btn" onClick={() => setConsent(false)}>
            Cancel
          </button>
        </div>
      </div>
    );
  }

  return (
    <div className="recorder">
      <button className="btn btn-primary" onClick={() => setConsent(true)} disabled={busy}>
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
  );
}
