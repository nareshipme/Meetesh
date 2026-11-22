import { useCallback, useEffect, useState } from "react";
import { api, DownloadPhase, ModelInfo, ModelLoadEvent, ModelSelection, ModelSlot } from "../api";
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
  const [logDir, setLogDir] = useState<string | null>(null);
  const [selection, setSelection] = useState<ModelSelection | null>(null);

  const refresh = useCallback(() => {
    api
      .listModels()
      .then(setModels)
      .catch((e) => notify("error", errorText(e)));
    api.modelSelection().then(setSelection).catch(() => {});
  }, [notify]);

  useEffect(() => {
    refresh();
    api.logDir().then(setLogDir).catch(() => {});
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

  /** Pick the model for a job. For notes, "" means "same as hints". */
  const choose = useCallback(
    async (slot: ModelSlot, id: string) => {
      try {
        setSelection(await api.selectModel(slot, id));
        refresh();
      } catch (e) {
        notify("error", errorText(e));
      }
    },
    [notify, refresh],
  );

  const used = (slot: "asr" | "assist" | "summary") => models.find((m) => m.selected_for.includes(slot));
  const speech = used("asr");
  const assist = used("assist");
  const notes = used("summary");
  return {
    selection,
    choose,
    notesInstalled: notes?.installed ?? false,
    /** Every model this device can use (all choices, not only the selected ones). */
    models,
    downloads,
    runtime,
    logDir,
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
