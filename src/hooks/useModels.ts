import { useCallback, useEffect, useState } from "react";
import { api, DownloadPhase, ModelInfo, ModelLoadEvent } from "../api";
import { errorText } from "../lib/format";
import type { Notify } from "./useToasts";

export interface Download {
  phase: DownloadPhase;
  downloaded: number;
  total: number;
  error: string | null;
}

/**
 * Everything about the models this device uses: what is installed, live download state
 * (progress, paused, failed) and load state (loading into memory, warming up, ready).
 */
export function useModels(notify: Notify) {
  const [models, setModels] = useState<ModelInfo[]>([]);
  const [downloads, setDownloads] = useState<Record<string, Download>>({});
  const [runtime, setRuntime] = useState<Record<string, ModelLoadEvent>>({});

  const refresh = useCallback(() => {
    api
      .listModels()
      .then((ms) => setModels(ms.filter((m) => m.is_default)))
      .catch((e) => notify("error", errorText(e)));
  }, [notify]);

  useEffect(() => {
    refresh();
    api
      .modelRuntime()
      .then((list) => setRuntime(Object.fromEntries(list.map((e) => [e.id, e]))))
      .catch(() => {});

    const unDownload = api.onDownload((e) => {
      if (e.phase === "done") {
        setDownloads(({ [e.id]: _gone, ...rest }) => rest);
      } else {
        setDownloads((d) => ({ ...d, [e.id]: { phase: e.phase, downloaded: e.downloaded, total: e.total, error: e.error } }));
      }
      if (e.phase !== "downloading" && e.phase !== "verifying") refresh();
    });
    const unLoad = api.onModelLoad((e) => setRuntime((r) => ({ ...r, [e.id]: e })));
    return () => {
      unDownload.then((f) => f());
      unLoad.then((f) => f());
    };
  }, [refresh]);

  const activeDownloadId = Object.entries(downloads).find(([, d]) => d.phase === "downloading" || d.phase === "verifying")?.[0] ?? null;

  const startDownload = useCallback(
    async (id: string) => {
      setDownloads((d) => ({ ...d, [id]: { phase: "downloading", downloaded: d[id]?.downloaded ?? 0, total: d[id]?.total ?? 0, error: null } }));
      try {
        await api.downloadModel(id);
      } catch (e) {
        setDownloads(({ [id]: _gone, ...rest }) => rest);
        notify("error", errorText(e));
      }
    },
    [notify],
  );

  const discard = useCallback(
    async (id: string) => {
      try {
        await api.discardDownload(id);
        setDownloads(({ [id]: _gone, ...rest }) => rest);
        refresh();
      } catch (e) {
        notify("error", errorText(e));
      }
    },
    [notify, refresh],
  );

  const speech = models.find((m) => m.role === "asr");
  const assist = models.find((m) => m.role === "assist");
  return {
    models,
    downloads,
    runtime,
    activeDownloadId,
    startDownload,
    discard,
    refresh,
    speechInstalled: speech?.installed ?? false,
    assistInstalled: assist?.installed ?? false,
    assistLoad: assist ? runtime[assist.id] : undefined,
  };
}

export type ModelsState = ReturnType<typeof useModels>;
