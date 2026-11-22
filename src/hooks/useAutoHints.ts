import { useCallback, useEffect, useState } from "react";
import { api, AutoHintMode } from "../api";
import { errorText } from "../lib/format";
import type { Notify } from "./useToasts";

/** The "automatic hints" setting (off / on questions / questions and pauses), kept by the backend. */
export function useAutoHints(notify: Notify) {
  const [mode, setMode] = useState<AutoHintMode>("off");
  useEffect(() => {
    api.getAutoHints().then(setMode).catch(() => {});
  }, []);
  const choose = useCallback(
    async (next: AutoHintMode) => {
      const before = mode;
      setMode(next);
      try {
        await api.setAutoHints(next);
      } catch (e) {
        setMode(before);
        notify("error", errorText(e));
      }
    },
    [mode, notify],
  );
  return { mode, choose };
}
