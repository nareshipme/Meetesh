import { useCallback, useEffect, useState } from "react";
import { api, ModelInfo } from "./api";

const mb = (b: number) => `${Math.round(b / 1_000_000)} MB`;

/** Speech model status and download. Models come from the manifest; there is no picker in v1. */
export default function ModelPanel({ onChange }: { onChange?: () => void }) {
  const [models, setModels] = useState<ModelInfo[]>([]);
  const [progress, setProgress] = useState<{ id: string; frac: number } | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(() => {
    api.listModels().then(setModels).catch((e) => setError(String(e)));
  }, []);

  useEffect(() => {
    refresh();
    const un = api.onDownload((e) => {
      if (e.done) {
        setProgress(null);
        if (e.error && e.error !== "cancelled") setError(e.error);
        refresh();
        onChange?.();
      } else {
        setProgress({ id: e.id, frac: e.total ? e.downloaded / e.total : 0 });
      }
    });
    return () => {
      un.then((f) => f());
    };
  }, [refresh, onChange]);

  const m = models.find((x) => x.is_default);
  if (!m) return null;

  async function download() {
    setError(null);
    setProgress({ id: m!.id, frac: 0 });
    try {
      await api.downloadModel(m!.id);
    } catch (e) {
      setProgress(null);
      setError(String(e));
    }
  }

  return (
    <div className="model-panel">
      <strong>Speech model</strong>
      <div>{m.name}</div>
      {m.installed ? (
        <small className="muted">Ready, runs on this device.</small>
      ) : progress ? (
        <>
          <progress value={progress.frac} max={1} aria-label="Download progress" />
          <button onClick={() => api.cancelDownload()}>Cancel</button>
        </>
      ) : (
        <>
          <small className="muted">Needed for transcription. One-time download, {mb(m.size_bytes)}.</small>
          <button onClick={download}>Download</button>
        </>
      )}
      {error && <small className="error">{error}</small>}
    </div>
  );
}
