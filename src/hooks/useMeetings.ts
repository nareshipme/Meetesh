import { useCallback, useEffect, useState } from "react";
import { api, Meeting } from "../api";
import { errorText } from "../lib/format";
import type { Notify } from "./useToasts";

/** Meetings plus what is recording and what is transcribing. */
export function useMeetings(notify: Notify) {
  const [meetings, setMeetings] = useState<Meeting[]>([]);
  const [recordingId, setRecordingId] = useState<number | null>(null);
  const [transcribingId, setTranscribingId] = useState<number | null>(null);
  
  const refresh = useCallback(async () => {
    try {
      const [list, rec] = await Promise.all([api.listMeetings(), api.activeRecording()]);
      setMeetings(list);
      setRecordingId(rec);
    } catch (e) {
      notify("error", errorText(e));
    }
  }, [notify]);

  useEffect(() => {
    refresh();
    api.activeTranscription().then(setTranscribingId).catch(() => {});
    const un = api.onTranscribe((e) => {
      if (e.done) {
        setTranscribingId(null);
        refresh();
      } else {
        setTranscribingId(e.meeting_id);
      }
    });
    return () => {
      un.then((f) => f());
    };
  }, [refresh]);

  return { meetings, recordingId, transcribingId, refresh };
}
