import type { ModelsState } from "../hooks/useModels";

const pct = (frac: number) => `${Math.round(frac * 100)}%`;

/** A slim bar at the top while installed models are being loaded into memory. */
export default function LoadingBanner({ ms }: { ms: ModelsState }) {
  const loading = ms.models.filter((m) => m.installed && (ms.runtime[m.id]?.state === "loading" || ms.runtime[m.id]?.state === "warming"));
  if (loading.length === 0) return null;

  return (
    <div className="loading-banner" role="status" aria-live="polite">
      <div className="loading-lines">
        {loading.map((m) => {
          const rt = ms.runtime[m.id];
          const known = rt.state === "loading" && rt.progress !== null;
          return (
            <div className="loading-line" key={m.id}>
              <span>
                {rt.state === "warming" ? "Warming up" : "Loading"} {m.name}
                {known ? ` · ${pct(rt.progress!)}` : "…"}
              </span>
              {known ? <progress value={rt.progress!} max={1} /> : <progress />}
            </div>
          );
        })}
      </div>
      <small className="muted">Getting ready, so nothing makes you wait later. You can already take notes.</small>
    </div>
  );
}
