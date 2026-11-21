import { useCallback, useEffect, useState } from "react";
import { api, Meeting } from "../api";
import { errorText } from "../lib/format";
import type { Notify } from "./useToasts";

/** Meetings plus the app-wide status the UI needs: what is recording, what is transcribing, is the model ready. */
export function useMeetings(notify: Notify) {
  const [meetings, setMeetings] = useState<Meeting[]>([]);
  const [recordingId, setRecordingId] = useState<number | null>(null);
  const [transcribingId, setTranscribingId] = useState<number | null>(null);
  const [modelReady, setModelReady] = useState(false); // speech model
  const [assistReady, setAssistReady] = useState(false); // live-assist model

  const refresh = useCallback(async () => {
    try {
      const [list, rec, models] = await Promise.all([api.listMeetings(), api.activeRecording(), api.listModels()]);
      setMeetings(list);
      setRecordingId(rec);
      setModelReady(models.some((m) => m.role === "asr" && m.is_default && m.installed));
      setAssistReady(models.some((m) => m.role === "assist" && m.is_default && m.installed));
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

  return { meetings, recordingId, transcribingId, modelReady, assistReady, refresh };
}
