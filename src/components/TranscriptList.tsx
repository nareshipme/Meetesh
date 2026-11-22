import type { ReactNode } from "react";
import type { TranscriptSegment } from "../api";
import { useFollowBottom } from "../hooks/useFollowBottom";
import { fmtDuration } from "../lib/format";

interface Props {
  segments: TranscriptSegment[];
  /** Follow the newest line (while recording or transcribing). */
  following: boolean;
  /** Shown instead of the list when there are no lines. */
  empty: ReactNode;
  /** Label lines You / Others (both sides of a call were recorded). */
  labelled: boolean;
}

const who = (s: TranscriptSegment) => (s.source === "system" ? "Others" : "You");

/** The transcript lines in a scrolling box that stays on the newest line while it grows. */
export default function TranscriptList({ segments, following, empty, labelled }: Props) {
  const follow = useFollowBottom<HTMLDivElement>(following, [segments]);
  return (
    <div className="transcript-wrap">
      <div className="transcript-body" ref={follow.ref} onScroll={follow.onScroll}>
        {segments.length > 0 ? (
          <ol className="segments">
            {segments.map((s, i) => (
              <li key={`${s.start_ms}-${i}`}>
                <time>{fmtDuration(s.start_ms)}</time>
                <span>
                  {labelled && <b className={`who ${s.source === "system" ? "who-others" : "who-you"}`}>{who(s)}</b>}
                  {s.text}
                </span>
              </li>
            ))}
          </ol>
        ) : (
          <p className="muted empty">{empty}</p>
        )}
      </div>
      {follow.behind && (
        <button className="btn btn-sm btn-primary jump-latest" onClick={follow.jump}>
          ↓ Latest
        </button>
      )}
    </div>
  );
}

export { who };
