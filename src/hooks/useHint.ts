import { useCallback, useEffect, useRef, useState } from "react";
import { api, HintSource, HintTrigger } from "../api";
import { errorText } from "../lib/format";
import type { Notify } from "./useToasts";

export type HintState =
  | { kind: "idle" }
  | { kind: "working"; text: string; trigger: HintTrigger } // text streams in as the model writes it
  | { kind: "done"; text: string; ms: number; backend: "GPU" | "CPU"; fellBack: boolean; trigger: HintTrigger; sources: HintSource[] }
  | { kind: "none" } // the model had nothing useful to say
  | { kind: "error"; message: string };

/** Asks the on-device model for a hint and follows its streamed answer. */
export function useHint(meetingId: number, notify: Notify) {
  const [state, setState] = useState<HintState>({ kind: "idle" });
  const currentId = useRef<number | null>(null);

  useEffect(() => {
    setState({ kind: "idle" });
    currentId.current = null;
    api
      .activeHint()
      .then((id) => id === meetingId && setState({ kind: "working", text: "", trigger: null }))
      .catch(() => {});

    const unPiece = api.onHintPiece((e) => {
      if (e.meeting_id !== meetingId) return;
      currentId.current = e.hint_id;
      setState({ kind: "working", text: e.text, trigger: e.trigger });
    });
    const unDone = api.onHintDone((e) => {
      if (e.meeting_id !== meetingId) return;
      currentId.current = null;
      if (e.trigger) {
        // A hint the app offered by itself: show it if there is one, and stay quiet otherwise
        // (no "nothing to suggest", no errors the user did not ask for).
        if (e.text && !e.error && !e.cancelled)
          setState({ kind: "done", text: e.text, ms: e.total_ms, backend: e.on_gpu ? "GPU" : "CPU", fellBack: e.cpu_fallback, trigger: e.trigger, sources: e.sources });
        else setState({ kind: "idle" });
      } else if (e.error) setState({ kind: "error", message: e.error });
      else if (e.cancelled) setState({ kind: "idle" });
      else if (e.text) setState({ kind: "done", text: e.text, ms: e.total_ms, backend: e.on_gpu ? "GPU" : "CPU", fellBack: e.cpu_fallback, trigger: null, sources: e.sources });
      else setState({ kind: "none" });
    });
    return () => {
      unPiece.then((f) => f());
      unDone.then((f) => f());
    };
  }, [meetingId]);

  const request = useCallback(async () => {
    setState({ kind: "working", text: "", trigger: null });
    try {
      await api.requestHint(meetingId);
    } catch (e) {
      setState({ kind: "idle" });
      notify("error", errorText(e));
    }
  }, [meetingId, notify]);

  const cancel = useCallback(() => {
    api.cancelHint().catch(() => {});
  }, []);

  const dismiss = useCallback(() => setState({ kind: "idle" }), []);

  return { state, request, cancel, dismiss };
}
