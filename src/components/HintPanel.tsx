import type { HintState } from "../hooks/useHint";

interface Props {
  state: HintState;
  onCancel: () => void;
  onDismiss: () => void;
  /** Open an earlier meeting the hint drew on. */
  onOpenMeeting?: (id: number) => void;
}

/** The live-assist hint: streams in as the model writes it, stays until dismissed. */
export default function HintPanel({ state, onCancel, onDismiss, onOpenMeeting }: Props) {
  if (state.kind === "idle") return null;

  return (
    <aside className={`hint hint-${state.kind}`} role="status" aria-live="polite" aria-label="Hint">
      <div className="hint-body">
        <span className="hint-icon" aria-hidden>
          ✦
        </span>
        <div className="hint-text">
          {state.kind === "working" &&
            (state.text ? (
              <>
                {state.text}
                <span className="caret" aria-hidden />
              </>
            ) : (
              <span className="muted">Thinking… (the first hint loads the model, which takes a few seconds)</span>
            ))}
          {state.kind === "done" && state.text}
          {state.kind === "done" && state.sources.length > 0 && (
            <div className="hint-sources">
              <small className="muted">The model also saw notes from:</small>
              {state.sources.map((s) => (
                <button key={s.meeting_id} className="source-chip" onClick={() => onOpenMeeting?.(s.meeting_id)} title="Open that meeting">
                  {s.title} · {s.day}
                </button>
              ))}
            </div>
          )}
          {state.kind === "none" && <span className="muted">Nothing useful to suggest right now.</span>}
          {state.kind === "error" && <span className="error-text">{state.message}</span>}
        </div>
      </div>
      <div className="hint-side">
        {(state.kind === "done" || state.kind === "working") && state.trigger && (
          <small className="chip chip-busy" title="Offered automatically. Change this with “Auto hints” above the transcript.">
            {state.trigger === "question" ? "after a question" : "during a pause"}
          </small>
        )}
        {state.kind === "done" && (
          <small className="muted" title={state.fellBack ? "The GPU could not run the model, so this hint used the CPU" : undefined}>
            {(state.ms / 1000).toFixed(1)} s · {state.backend}
            {state.fellBack ? " (fallback)" : ""}
          </small>
        )}
        {state.kind === "working" ? (
          <button className="btn btn-sm" onClick={onCancel}>
            Stop
          </button>
        ) : (
          <button className="hint-close" aria-label="Dismiss hint" onClick={onDismiss}>
            ×
          </button>
        )}
      </div>
    </aside>
  );
}
