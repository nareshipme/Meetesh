import type { useLivePoints } from "../hooks/useLivePoints";

interface Props {
  points: ReturnType<typeof useLivePoints>;
  assistReady: boolean;
}

/** What is being discussed right now, and the points so far; updates itself while recording. */
export default function LivePointsPanel({ points, assistReady }: Props) {
  return (
    <section className="points" aria-label="Live points">
      <div className="points-head">
        <h3>Now discussing</h3>
        <label className="check small" title="A short list of what is being discussed, updated about once a minute. It uses the processor a little; switch it off if live text lags.">
          <input type="checkbox" checked={points.enabled} onChange={(e) => points.setEnabled(e.target.checked)} />
          On
        </label>
      </div>
      {!points.enabled ? (
        <p className="muted small">Live points are off.</p>
      ) : !assistReady ? (
        <p className="muted small">Needs a live assist model (Settings).</p>
      ) : points.now || points.points.length > 0 ? (
        <>
          {points.now && <p className="points-now">{points.now}</p>}
          {points.points.length > 0 && (
            <ul className="points-list">
              {points.points.map((p, i) => (
                <li key={`${i}-${p}`}>{p}</li>
              ))}
            </ul>
          )}
        </>
      ) : (
        <p className="muted small">The list appears after about a minute of conversation.</p>
      )}
      {points.enabled && points.updating && (
        <small className="muted" aria-live="polite">
          Updating…
        </small>
      )}
    </section>
  );
}
