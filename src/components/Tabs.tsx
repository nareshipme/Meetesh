import type { ReactNode } from "react";

export interface Tab {
  id: string;
  label: string;
  /** A small count or dot after the label. */
  badge?: ReactNode;
}

interface Props {
  tabs: Tab[];
  active: string;
  onChange: (id: string) => void;
  label: string;
}

/** Accessible tab strip: arrow keys move between tabs, Home/End jump to the ends. */
export default function Tabs({ tabs, active, onChange, label }: Props) {
  function onKey(e: React.KeyboardEvent) {
    const i = tabs.findIndex((t) => t.id === active);
    let next = i;
    if (e.key === "ArrowRight") next = (i + 1) % tabs.length;
    else if (e.key === "ArrowLeft") next = (i - 1 + tabs.length) % tabs.length;
    else if (e.key === "Home") next = 0;
    else if (e.key === "End") next = tabs.length - 1;
    else return;
    e.preventDefault();
    onChange(tabs[next].id);
    document.getElementById(`tab-${tabs[next].id}`)?.focus();
  }
  return (
    <div className="tabs" role="tablist" aria-label={label} onKeyDown={onKey}>
      {tabs.map((t) => (
        <button
          key={t.id}
          id={`tab-${t.id}`}
          role="tab"
          aria-selected={t.id === active}
          aria-controls={`panel-${t.id}`}
          tabIndex={t.id === active ? 0 : -1}
          className={t.id === active ? "tab active" : "tab"}
          onClick={() => onChange(t.id)}
        >
          {t.label}
          {t.badge}
        </button>
      ))}
    </div>
  );
}
