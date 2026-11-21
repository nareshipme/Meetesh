import { useCallback, useEffect, useState } from "react";
import { api, ModelInfo } from "../api";
import type { Notify } from "../hooks/useToasts";
import { errorText } from "../lib/format";

const mb = (b: number) => `${Math.round(b / 1_000_000)} MB`;

/** Speech model status and download. Models come from the manifest; there is no picker in v1. */
export default function ModelPanel({ notify, onChange }: { notify: Notify; onChange: () => void }) {
  const [model, setModel] = useState<ModelInfo | null>(null);
  const [progress, setProgress] = useState<number | null>(null);

  const refresh = useCallback(() => {
    api
      .listModels()
      .then((ms) => setModel(ms.find((m) => m.is_default) ?? null))
      .catch((e) => notify("error", errorText(e)));
  }, [notify]);

  useEffect(() => {
    refresh();
    const un = api.onDownload((e) => {
      if (e.done) {
        setProgress(null);
        if (e.error && e.error !== "cancelled") notify("error", e.error);
        refresh();
        onChange();
      } else {
        setProgress(e.total ? e.downloaded / e.total : 0);
      }
    });
    return () => {
      un.then((f) => f());
    };
  }, [refresh, onChange, notify]);

  if (!model) return null;

  async function download() {
    setProgress(0);
    try {
      await api.downloadModel(model!.id);
    } catch (e) {
      setProgress(null);
      notify("error", errorText(e));
    }
  }

  return (
    <div className="model-panel">
      <div className="model-head">
        <span className={`dot ${model.installed ? "dot-ok" : "dot-off"}`} aria-hidden />
        <div>
          <div className="model-name">{model.name}</div>
          <small className="muted">
            {model.installed ? "Ready. Runs on this device." : `Speech model, ${mb(model.size_bytes)} one-time download`}
          </small>
        </div>
      </div>
      {!model.installed && progress === null && (
        <button className="btn btn-primary btn-block" onClick={download}>
          Download model
        </button>
      )}
      {progress !== null && (
        <div className="row">
          <progress value={progress} max={1} aria-label="Download progress" />
          <span className="muted">{Math.round(progress * 100)}%</span>
          <button className="btn btn-sm" onClick={() => api.cancelDownload()}>
            Cancel
          </button>
        </div>
      )}
    </div>
  );
}
