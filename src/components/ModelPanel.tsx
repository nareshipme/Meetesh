import { useCallback, useEffect, useState } from "react";
import { api, ModelInfo } from "../api";
import type { Notify } from "../hooks/useToasts";
import { errorText } from "../lib/format";

const size = (b: number) => (b >= 1e9 ? `${(b / 1e9).toFixed(1)} GB` : `${Math.round(b / 1e6)} MB`);

const ROLE_LABEL: Record<ModelInfo["role"], string> = { asr: "Speech to text", assist: "Live assist (optional)" };

/** The models this device uses (speech, live assist) and their download state. Models come from the manifest; there is no picker in v1. */
export default function ModelPanel({ notify, onChange }: { notify: Notify; onChange: () => void }) {
  const [models, setModels] = useState<ModelInfo[]>([]);
  const [downloading, setDownloading] = useState<{ id: string; frac: number } | null>(null);

  const refresh = useCallback(() => {
    api
      .listModels()
      .then((ms) => setModels(ms.filter((m) => m.is_default)))
      .catch((e) => notify("error", errorText(e)));
  }, [notify]);

  useEffect(() => {
    refresh();
    const un = api.onDownload((e) => {
      if (e.done) {
        setDownloading(null);
        if (e.error && e.error !== "cancelled") notify("error", e.error);
        refresh();
        onChange();
      } else {
        setDownloading({ id: e.id, frac: e.total ? e.downloaded / e.total : 0 });
      }
    });
    return () => {
      un.then((f) => f());
    };
  }, [refresh, onChange, notify]);

  async function download(id: string) {
    setDownloading({ id, frac: 0 });
    try {
      await api.downloadModel(id);
    } catch (e) {
      setDownloading(null);
      notify("error", errorText(e));
    }
  }

  if (models.length === 0) return null;

  return (
    <div className="model-panel">
      {models.map((m) => {
        const busy = downloading?.id === m.id;
        return (
          <div className="model-row" key={m.id}>
            <div className="model-head">
              <span className={`dot ${m.installed ? "dot-ok" : "dot-off"}`} aria-hidden />
              <div>
                <div className="model-name">{m.name}</div>
                <small className="muted">
                  {ROLE_LABEL[m.role]} ·{" "}
                  {m.installed ? "ready, runs on this device" : `${size(m.size_bytes)} one-time download`}
                </small>
              </div>
            </div>
            {!m.installed && !busy && (
              <button className="btn btn-sm btn-block" onClick={() => download(m.id)} disabled={downloading !== null}>
                Download
              </button>
            )}
            {busy && (
              <div className="row">
                <progress value={downloading!.frac} max={1} aria-label={`Downloading ${m.name}`} />
                <span className="muted">{Math.round(downloading!.frac * 100)}%</span>
                <button className="btn btn-sm" onClick={() => api.cancelDownload()}>
                  Cancel
                </button>
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
}
