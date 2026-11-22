import type { SearchMatch, SearchResult } from "../api";
import { fmtDuration, fmtWhen } from "../lib/format";

interface Props {
  query: string;
  results: SearchResult[] | null;
  error: string | null;
  selectedId: number | null;
  onSelect: (id: number) => void;
}

const START = "\u0001";
const END = "\u0002";

/** Shows a snippet with the matched words in bold. The markers are control characters, never real text. */
function Highlighted({ text }: { text: string }) {
  const parts: JSX.Element[] = [];
  let i = 0;
  for (const chunk of text.split(START)) {
    const [hit, rest] = chunk.includes(END) ? chunk.split(END) : [null, chunk];
    if (hit !== null) parts.push(<mark key={i++}>{hit}</mark>);
    if (rest) parts.push(<span key={i++}>{rest}</span>);
  }
  return <>{parts}</>;
}

/** Notes are Markdown; in a one-line snippet the "##" and "-" markers are just noise. */
function plain(text: string): string {
  return text.replace(/(^|\s)#{1,6}\s+/g, "$1").replace(/(^|\s)-\s+/g, "$1");
}

function where(m: SearchMatch): string {
  if (m.kind === "notes") return "Notes";
  if (m.kind === "transcript") return fmtDuration(m.start_ms ?? 0);
  return "";
}

/** Replaces the meeting list while a search is active. */
export default function SearchResults({ query, results, error, selectedId, onSelect }: Props) {
  if (error) return <p className="muted pad">Search failed: {error}</p>;
  if (results === null) return <p className="muted pad">Searching…</p>;
  if (results.length === 0) return <p className="muted pad">No meeting matches “{query.trim()}”.</p>;
  return (
    <ul className="results" aria-label="Search results">
      {results.map((r) => (
        <li key={r.meeting_id}>
          <button className={r.meeting_id === selectedId ? "item active" : "item"} onClick={() => onSelect(r.meeting_id)}>
            <span className="item-title">
              {r.matches.find((m) => m.kind === "title") ? <Highlighted text={r.matches.find((m) => m.kind === "title")!.snippet} /> : r.title}
            </span>
            <span className="item-meta">{fmtWhen(r.created_at)}</span>
            {r.matches
              .filter((m) => m.kind !== "title")
              .map((m, i) => (
                <span className="hit" key={i}>
                  <small className="hit-where">{where(m)}</small>
                  <Highlighted text={m.kind === "notes" ? plain(m.snippet) : m.snippet} />
                </span>
              ))}
          </button>
        </li>
      ))}
    </ul>
  );
}
