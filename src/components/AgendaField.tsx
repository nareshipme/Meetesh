import { useEffect, useState } from "react";
import { api, Meeting } from "../api";
import type { Notify } from "../hooks/useToasts";
import { errorText } from "../lib/format";

/** Optional goals or agenda. The live-assist model reads it so hints stay on topic. */
export default function AgendaField({ meeting, notify, onChange }: { meeting: Meeting; notify: Notify; onChange: () => void }) {
  const [text, setText] = useState(meeting.agenda);

  useEffect(() => setText(meeting.agenda), [meeting.id, meeting.agenda]);

  async function save() {
    if (text === meeting.agenda) return;
    try {
      await api.setAgenda(meeting.id, text);
      onChange();
    } catch (e) {
      notify("error", errorText(e));
    }
  }

  return (
    <details className="agenda" open={meeting.agenda.length > 0 || undefined}>
      <summary>Meeting goals (optional, helps hints stay on topic)</summary>
      <textarea
        aria-label="Meeting goals"
        placeholder="For example: decide the launch date; confirm who owns the pricing page"
        value={text}
        rows={3}
        maxLength={4000}
        onChange={(e) => setText(e.target.value)}
        onBlur={save}
      />
    </details>
  );
}
