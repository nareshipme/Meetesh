import { useCallback, useEffect, useState } from "react";
import { api, LivePointsState } from "../api";
import { errorText } from "../lib/format";
import type { Notify } from "./useToasts";

const EMPTY: LivePointsState = { meeting_id: 0, enabled: true, now: "", points: [], updating: false };

/**
 * The live list of what is being discussed. `meetingId`: only that recording's list is shown;
 * pass 0 where only the on/off switch matters (Settings).
 */
export function useLivePoints(meetingId: number, notify: Notify) {
  const [state, setState] = useState<LivePointsState>(EMPTY);

  useEffect(() => {
    api.getLivePoints().then((s) => setState(s.meeting_id === meetingId || meetingId === 0 ? s : { ...EMPTY, enabled: s.enabled })).catch(() => {});
    const un = api.onLivePoints((e) => {
      // The switch is global; the list belongs to one recording.
      setState((prev) => (e.meeting_id === meetingId || meetingId === 0 ? e : { ...prev, enabled: e.enabled }));
    });
    return () => {
      un.then((f) => f());
    };
  }, [meetingId]);

  const setEnabled = useCallback(
    async (on: boolean) => {
      setState((s) => ({ ...s, enabled: on }));
      try {
        await api.setLivePoints(on);
      } catch (e) {
        setState((s) => ({ ...s, enabled: !on }));
        notify("error", errorText(e));
      }
    },
    [notify],
  );

  return { ...state, setEnabled };
}
