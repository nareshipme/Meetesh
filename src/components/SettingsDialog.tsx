import { useEffect, useRef } from "react";
import type { useAutoHints } from "../hooks/useAutoHints";
import type { ModelsState } from "../hooks/useModels";
import ModelSettings from "./ModelSettings";
import type { AutoHintMode } from "../api";

interface Props {
  ms: ModelsState;
  auto: ReturnType<typeof useAutoHints>;
  onClose: () => void;
}

/** Everything you set up once: which models to use, automatic hints, and where things are. */
export default function SettingsDialog({ ms, auto, onClose }: Props) {
  const ref = useRef<HTMLDivElement>(null);

  // Focus the dialog, close on Escape, and give focus back to whatever opened it.
  useEffect(() => {
    const before = document.activeElement as HTMLElement | null;
    ref.current?.focus();
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") onClose();
    }
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("keydown", onKey);
      before?.focus?.();
    };
  }, [onClose]);

  return (
    <div className="modal-backdrop" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="modal" role="dialog" aria-modal="true" aria-label="Settings" tabIndex={-1} ref={ref}>
        <header className="modal-head">
          <h2>Settings</h2>
          <button className="btn btn-sm" onClick={onClose} aria-label="Close settings">
            Close
          </button>
        </header>

        <section>
          <h3 className="section-title">Models</h3>
          <p className="muted small">Models run on this device. Each job can use a different one; downloads happen once.</p>
          <ModelSettings ms={ms} />
        </section>

        <section>
          <h3 className="section-title">Hints</h3>
          <label className="field">
            <span>Automatic hints</span>
            <select value={auto.mode} onChange={(e) => auto.choose(e.target.value as AutoHintMode)}>
              <option value="off">Off</option>
              <option value="questions">On questions from the other participants</option>
              <option value="questions_and_pauses">On questions and after pauses</option>
            </select>
          </label>
          <p className="muted small">Questions need the other participants recorded (system audio). Pauses work with a microphone alone.</p>
        </section>

        <section>
          <h3 className="section-title">About</h3>
          <p className="muted small">
            Meetesh runs on this device. Audio and transcripts are not sent anywhere.
            {ms.logDir && <> Log folder: <code>{ms.logDir}</code></>}
          </p>
        </section>
      </div>
    </div>
  );
}
