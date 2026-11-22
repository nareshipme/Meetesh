import type { AutoHintMode, Meeting } from "../api";
import type { useAutoHints } from "../hooks/useAutoHints";
import type { useHint } from "../hooks/useHint";
import type { Notify } from "../hooks/useToasts";
import AgendaField from "./AgendaField";
import HintPanel from "./HintPanel";

interface Props {
  meeting: Meeting;
  hint: ReturnType<typeof useHint>;
  auto: ReturnType<typeof useAutoHints>;
  assistReady: boolean;
  /** Why asking for a hint is not possible right now ("" when it is). */
  hintBlockedReason: string;
  notify: Notify;
  onChange: () => void;
  onOpenSettings: () => void;
  onOpenMeeting: (id: number) => void;
}

/** The side column of the live screen: hints, and the goals they follow. */
export default function LiveAssist({ meeting, hint, auto, assistReady, hintBlockedReason, notify, onChange, onOpenSettings, onOpenMeeting }: Props) {
  return (
    <section className="assist" aria-label="Live assist">
      <div className="assist-head">
        <h2>Live assist</h2>
        <select
          className="auto-hints"
          aria-label="Automatic hints"
          value={auto.mode}
          disabled={!assistReady}
          onChange={(e) => auto.choose(e.target.value as AutoHintMode)}
          title={
            assistReady
              ? "Offer a hint by itself. Questions are noticed when the other participants are recorded (system audio); pauses work with a microphone alone."
              : "Download a live assist model first (Settings)"
          }
        >
          <option value="off">Auto hints: off</option>
          <option value="questions">Auto hints: on questions</option>
          <option value="questions_and_pauses">Auto hints: questions and pauses</option>
        </select>
      </div>

      <button
        className="btn btn-hint btn-block"
        onClick={hint.request}
        disabled={hintBlockedReason !== ""}
        title={hintBlockedReason || "Get a hint from the last few minutes (Ctrl/Cmd+Shift+H)"}
      >
        ✦ Hint
      </button>

      {hint.state.kind === "idle" ? (
        <p className="muted small assist-empty">
          {assistReady ? (
            "Hints appear here. Ask for one, or turn on automatic hints."
          ) : (
            <>
              Hints need a live assist model.{" "}
              <button className="link-btn inline" onClick={onOpenSettings}>
                Set it up in Settings
              </button>
            </>
          )}
        </p>
      ) : (
        <HintPanel state={hint.state} onCancel={hint.cancel} onDismiss={hint.dismiss} onOpenMeeting={onOpenMeeting} />
      )}

      <AgendaField meeting={meeting} notify={notify} onChange={onChange} />
    </section>
  );
}
