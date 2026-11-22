import { useCallback, useEffect, useState } from "react";
import { api, Meeting, TranscriptSegment } from "../api";
import { errorText } from "../lib/format";
import type { Notify } from "./useToasts";

/** The transcript of one meeting: saved lines, lines arriving live, and a transcription run in progress. */
export function useTranscript(meeting: Meeting, notify: Notify, onChange: () => void) {
  const [segments, setSegments] = useState<TranscriptSegment[]>([]);
  const [progress, setProgress] = useState<number | null>(null);

  const load = useCallback(() => {
    api.getTranscript(meeting.id).then(setSegments).catch((e) => notify("error", errorText(e)));
  }, [meeting.id, notify]);

  useEffect(() => {
    load();
    api.activeTranscription().then((id) => setProgress(id === meeting.id ? 0 : null)).catch(() => {});
    const unProgress = api.onTranscribe((e) => {
      if (e.meeting_id !== meeting.id) return;
      if (e.done) {
        setProgress(null);
        if (e.error) notify("error", e.error);
        load();
        onChange();
      } else {
        setProgress(e.progress);
      }
    });
    // Live transcription: text arrives while the meeting is being recorded.
    const unLive = api.onLiveSegments((e) => {
      if (e.meeting_id === meeting.id) setSegments((prev) => [...prev, ...e.segments]);
    });
    const unDone = api.onLiveDone((e) => {
      if (e.meeting_id !== meeting.id) return;
      load(); // pick up the result of a gap-filling pass, if one ran
      onChange();
    });
    return () => {
      unProgress.then((f) => f());
      unLive.then((f) => f());
      unDone.then((f) => f());
    };
  }, [meeting.id, load, notify, onChange]);

  const run = useCallback(async () => {
    try {
      setProgress(0);
      await api.transcribeMeeting(meeting.id);
    } catch (e) {
      setProgress(null);
      notify("error", errorText(e));
    }
  }, [meeting.id, notify]);

  return { segments, progress, run, transcribing: progress !== null };
}
