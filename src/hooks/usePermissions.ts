import { useCallback, useEffect, useState } from "react";
import { api, Permission } from "../api";
import { errorText } from "../lib/format";

/**
 * The permissions list. It is read again whenever the window gets focus back, because the person
 * has usually just been in the system's settings changing something.
 */
export function usePermissions() {
  const [list, setList] = useState<Permission[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      setList(await api.listPermissions());
    } catch (e) {
      setError(errorText(e));
    }
  }, []);

  useEffect(() => {
    refresh();
    window.addEventListener("focus", refresh);
    return () => window.removeEventListener("focus", refresh);
  }, [refresh]);

  /** Run an action, show its error, and re-read the list (the system may take a moment to change). */
  const act = useCallback(
    async (f: () => Promise<unknown>) => {
      setError(null);
      try {
        await f();
      } catch (e) {
        setError(errorText(e));
      }
      await refresh();
      window.setTimeout(refresh, 1500); // the answer to a system prompt arrives a little later
    },
    [refresh],
  );

  return {
    list,
    error,
    refresh,
    request: (id: string) => act(() => api.requestPermission(id)),
    openSettings: (id: string) => act(() => api.openSystemSettings(id)),
    setSwitch: (id: string, on: boolean) => act(() => api.setPermissionSwitch(id, on)),
    removeKey: () => act(() => api.clearOnlineKey()),
  };
}
