import { useCallback, useEffect, useState } from "react";
import { api, OnlineSettings, OnlineStatus } from "../api";
import { errorText } from "../lib/format";

type Outcome<T> = { ok: true; value: T } | { ok: false };

/** The optional online model for meeting notes: what is saved, and the actions on it. */
export function useOnline() {
  const [status, setStatus] = useState<OnlineStatus | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api.onlineStatus().then(setStatus).catch((e) => setError(errorText(e)));
  }, []);

  /** Run a call; a failure becomes the visible error instead of an exception. */
  const run = useCallback(async <T,>(f: () => Promise<T>): Promise<Outcome<T>> => {
    setError(null);
    try {
      return { ok: true, value: await f() };
    } catch (e) {
      setError(errorText(e));
      return { ok: false };
    }
  }, []);

  /** A call that returns the new status. */
  const update = useCallback(
    async (f: () => Promise<OnlineStatus>) => {
      const r = await run(f);
      if (r.ok) setStatus(r.value);
      return r.ok;
    },
    [run],
  );

  return {
    status,
    error,
    save: (s: OnlineSettings, consent: boolean) => update(() => api.saveOnline(s, consent)),
    setKey: (key: string) => update(() => api.setOnlineKey(key)),
    clearKey: () => update(() => api.clearOnlineKey()),
    test: async (s: OnlineSettings) => (await run(() => api.testOnline(s))).ok,
    models: async (s: OnlineSettings) => {
      const r = await run(() => api.listOnlineModels(s));
      return r.ok ? r.value : null;
    },
  };
}
