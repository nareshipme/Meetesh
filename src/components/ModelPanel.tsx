import { api, ModelInfo } from "../api";
import type { ModelsState } from "../hooks/useModels";

const size = (b: number) => (b >= 1e9 ? `${(b / 1e9).toFixed(1)} GB` : `${Math.round(b / 1e6)} MB`);
const pct = (frac: number) => `${Math.round(frac * 100)}%`;

const ROLE_LABEL: Record<ModelInfo["role"], string> = { asr: "Speech to text", assist: "Live assist (optional)" };

function Row({ m, ms }: { m: ModelInfo; ms: ModelsState }) {
  const dl = ms.downloads[m.id];
  const rt = ms.runtime[m.id];
  const otherBusy = ms.activeDownloadId !== null && ms.activeDownloadId !== m.id;
  const partial = dl?.downloaded ?? m.partial_bytes;

  let dot = "dot-off";
  let status: React.ReactNode;
  let controls: React.ReactNode = null;

  if (!m.installed) {
    if (dl?.phase === "downloading") {
      dot = "dot-busy";
      const frac = dl.total ? dl.downloaded / dl.total : 0;
      status = (
        <>
          <progress value={frac} max={1} aria-label={`Downloading ${m.name}`} />
          <small className="muted">
            Downloading {pct(frac)} · {size(dl.downloaded)} of {size(m.size_bytes)}
          </small>
        </>
      );
      controls = (
        <button className="btn btn-sm" onClick={() => api.cancelDownload()} title="Stop for now; you can resume later">
          Pause
        </button>
      );
    } else if (dl?.phase === "verifying") {
      dot = "dot-busy";
      status = (
        <>
          <progress aria-label="Verifying download" />
          <small className="muted">Verifying the download…</small>
        </>
      );
    } else if (dl?.phase === "failed") {
      dot = "dot-bad";
      status = (
        <small className="error-text">
          Download failed{partial > 0 ? ` at ${pct(partial / m.size_bytes)}` : ""}: {dl.error}
        </small>
      );
      controls = (
        <>
          <button className="btn btn-sm btn-primary" onClick={() => ms.startDownload(m.id)} disabled={otherBusy}>
            Retry
          </button>
          {partial > 0 && (
            <button className="btn btn-sm" onClick={() => ms.discard(m.id)}>
              Discard
            </button>
          )}
        </>
      );
    } else if (partial > 0) {
      status = (
        <small className="muted">
          Paused at {pct(partial / m.size_bytes)} · {size(partial)} of {size(m.size_bytes)}
        </small>
      );
      controls = (
        <>
          <button className="btn btn-sm btn-primary" onClick={() => ms.startDownload(m.id)} disabled={otherBusy}>
            Resume
          </button>
          <button className="btn btn-sm" onClick={() => ms.discard(m.id)}>
            Discard
          </button>
        </>
      );
    } else {
      status = (
        <small className="muted">
          {ROLE_LABEL[m.role]} · {size(m.size_bytes)} one-time download
        </small>
      );
      controls = (
        <button className="btn btn-sm btn-primary" onClick={() => ms.startDownload(m.id)} disabled={otherBusy}>
          Download
        </button>
      );
    }
  } else if (rt?.state === "loading") {
    dot = "dot-busy";
    status = (
      <>
        {rt.progress !== null ? <progress value={rt.progress} max={1} aria-label={`Loading ${m.name}`} /> : <progress aria-label={`Loading ${m.name}`} />}
        <small className="muted">Loading into memory{rt.progress !== null ? ` ${pct(rt.progress)}` : ""}…</small>
      </>
    );
  } else if (rt?.state === "warming") {
    dot = "dot-busy";
    status = (
      <>
        <progress aria-label={`Warming up ${m.name}`} />
        <small className="muted">Warming up…</small>
      </>
    );
  } else if (rt?.state === "failed") {
    dot = "dot-bad";
    status = <small className="error-text">Could not load: {rt.error}</small>;
    controls = (
      <button className="btn btn-sm btn-primary" onClick={() => api.retryModelLoad(m.id)}>
        Retry
      </button>
    );
  } else if (rt?.state === "ready") {
    dot = "dot-ok";
    status = (
      <small className="muted">
        {ROLE_LABEL[m.role]} · ready{rt.backend ? ` · ${rt.backend}` : ""}
      </small>
    );
  } else {
    dot = "dot-busy";
    status = <small className="muted">Installed · starting to load…</small>;
  }

  return (
    <div className="model-row">
      <div className="model-head">
        <span className={`dot ${dot}`} aria-hidden />
        <div className="model-info">
          <div className="model-name">{m.name}</div>
          {status}
        </div>
      </div>
      {controls && <div className="row model-controls">{controls}</div>}
    </div>
  );
}

/** The models this device uses (speech, live assist): download, pause/resume/retry, and loading progress. */
export default function ModelPanel({ ms }: { ms: ModelsState }) {
  if (ms.models.length === 0) return null;
  return (
    <div className="model-panel">
      {ms.models.map((m) => (
        <Row key={m.id} m={m} ms={ms} />
      ))}
    </div>
  );
}
