import type { ModelInfo, ModelSlot } from "../api";
import type { ModelsState } from "../hooks/useModels";
import ModelRow from "./ModelRow";

const size = (b: number) => (b >= 1e9 ? `${(b / 1e9).toFixed(1)} GB` : `${Math.round(b / 1e6)} MB`);

interface SlotInfo {
  slot: ModelSlot;
  title: string;
  help: string;
}

const SLOTS: SlotInfo[] = [
  { slot: "asr", title: "Speech to text", help: "Turns audio into the transcript. Bigger is more accurate and slower." },
  { slot: "assist", title: "Hints", help: "Writes live hints. Smaller is faster; faster matters most here." },
  { slot: "summary", title: "Meeting notes and titles", help: "Runs in the background after a meeting, so a slower, better model is fine." },
];

const label = (m: ModelInfo) => `${m.name} · ${size(m.size_bytes)}${m.installed ? " · installed" : ""}`;

/** Pick the model for each job, and see its download or load state. */
export default function ModelSettings({ ms }: { ms: ModelsState }) {
  return (
    <div className="slots">
      {SLOTS.map(({ slot, title, help }) => {
        const kind = slot === "asr" ? "asr" : "assist";
        const choices = ms.models.filter((m) => m.role === kind);
        if (choices.length === 0) return null;
        const selectedId = ms.selection ? ms.selection[slot] : "";
        const selected = choices.find((m) => m.id === selectedId);
        const follows = slot === "summary" && ms.selection?.summary_follows_hints;
        // For notes, the row below shows whichever model is really used.
        const shown = selected ?? choices[0];
        return (
          <section className="slot" key={slot} aria-label={title}>
            <div className="slot-head">
              <h3>{title}</h3>
              <select
                aria-label={`Model for ${title}`}
                value={follows ? "" : selectedId}
                onChange={(e) => ms.choose(slot, e.target.value)}
              >
                {slot === "summary" && <option value="">Same as hints{selected ? ` (${selected.name})` : ""}</option>}
                {choices.map((m) => (
                  <option key={m.id} value={m.id}>
                    {label(m)}
                  </option>
                ))}
              </select>
            </div>
            <p className="muted small">{help}</p>
            <ModelRow m={shown} ms={ms} />
            {shown.description && <p className="muted small">{shown.description}</p>}
          </section>
        );
      })}
    </div>
  );
}
