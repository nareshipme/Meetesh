import { useCallback, useRef, useState } from "react";

export type ToastKind = "info" | "error";
export interface Toast {
  id: number;
  kind: ToastKind;
  text: string;
}
export type Notify = (kind: ToastKind, text: string) => void;

/** Small notification queue. Info toasts disappear on their own; errors stay until dismissed. */
export function useToasts() {
  const [toasts, setToasts] = useState<Toast[]>([]);
  const nextId = useRef(1);

  const dismiss = useCallback((id: number) => setToasts((t) => t.filter((x) => x.id !== id)), []);

  const notify: Notify = useCallback(
    (kind, text) => {
      const id = nextId.current++;
      setToasts((t) => [...t.filter((x) => x.text !== text), { id, kind, text }]);
      if (kind === "info") setTimeout(() => dismiss(id), 6000);
    },
    [dismiss],
  );

  return { toasts, notify, dismiss };
}
