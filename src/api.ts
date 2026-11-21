import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export interface Meeting {
  id: number;
  title: string;
  created_at: number; // unix seconds
  summary: string;
  transcript_model: string;
  /** Goals typed by the user; context for live hints. */
  agenda: string;
  status: "stopped" | "recording" | "recovered";
  duration_ms: number;
  audio_dir: string;
}

export interface InputDevice {
  name: string;
  is_default: boolean;
}

export interface TranscriptSegment {
  start_ms: number;
  end_ms: number;
  text: string;
}

export interface ModelInfo {
  id: string;
  /** "asr" = speech recognition, "assist" = live hints */
  role: "asr" | "assist";
  name: string;
  description: string;
  size_bytes: number;
  license: string;
  installed: boolean;
  is_default: boolean;
}

export interface DownloadEvent {
  id: string;
  downloaded: number;
  total: number;
  done: boolean;
  error: string | null;
}

export interface TranscribeEvent {
  meeting_id: number;
  progress: number;
  done: boolean;
  cancelled: boolean;
  error: string | null;
}

export interface LiveSegmentsEvent {
  meeting_id: number;
  segments: TranscriptSegment[];
}

export interface LiveDoneEvent {
  meeting_id: number;
  full_pass_started: boolean;
}

export interface HintPieceEvent {
  meeting_id: number;
  hint_id: number;
  /** Everything generated so far (raw). */
  text: string;
}

export interface HintDoneEvent {
  meeting_id: number;
  hint_id: number;
  /** Cleaned hint; null when the model had nothing useful to say. */
  text: string | null;
  cancelled: boolean;
  error: string | null;
  first_token_ms: number;
  total_ms: number;
  tokens: number;
}

// Thin typed wrapper over the Rust commands. Keep all invoke() calls here so the
// UI never depends on Tauri directly (easier to mock in tests, and for mobile).
export const api = {
  listMeetings: () => invoke<Meeting[]>("list_meetings"),
  createMeeting: (title?: string) => invoke<Meeting>("create_meeting", { title }),
  renameMeeting: (id: number, title: string) => invoke<Meeting>("rename_meeting", { id, title }),
  deleteMeeting: (id: number) => invoke<void>("delete_meeting", { id }),
  listInputDevices: () => invoke<InputDevice[]>("list_input_devices"),
  activeRecording: () => invoke<number | null>("active_recording"),
  startRecording: (meetingId: number, device?: string) =>
    invoke<Meeting>("start_recording", { meetingId, device }),
  stopRecording: () => invoke<Meeting>("stop_recording"),
  exportAudio: (id: number) => invoke<string>("export_audio", { id }),
  listModels: () => invoke<ModelInfo[]>("list_models"),
  downloadModel: (id: string) => invoke<void>("download_model", { id }),
  cancelDownload: () => invoke<void>("cancel_download"),
  deleteModel: (id: string) => invoke<void>("delete_model", { id }),
  getTranscript: (id: number) => invoke<TranscriptSegment[]>("get_transcript", { id }),
  transcribeMeeting: (id: number) => invoke<void>("transcribe_meeting", { id }),
  cancelTranscription: () => invoke<void>("cancel_transcription"),
  activeTranscription: () => invoke<number | null>("active_transcription"),
  onDownload: (cb: (e: DownloadEvent) => void) => listen<DownloadEvent>("model-download", (e) => cb(e.payload)),
  onTranscribe: (cb: (e: TranscribeEvent) => void) =>
    listen<TranscribeEvent>("transcribe-progress", (e) => cb(e.payload)),
  onLiveSegments: (cb: (e: LiveSegmentsEvent) => void) =>
    listen<LiveSegmentsEvent>("live-segments", (e) => cb(e.payload)),
  onLiveDone: (cb: (e: LiveDoneEvent) => void) => listen<LiveDoneEvent>("live-done", (e) => cb(e.payload)),
  setAgenda: (id: number, agenda: string) => invoke<Meeting>("set_agenda", { id, agenda }),
  requestHint: (meetingId: number) => invoke<number>("request_hint", { meetingId }),
  cancelHint: () => invoke<void>("cancel_hint"),
  assistReady: () => invoke<boolean>("assist_ready"),
  activeHint: () => invoke<number | null>("active_hint"),
  onHintPiece: (cb: (e: HintPieceEvent) => void) => listen<HintPieceEvent>("hint-piece", (e) => cb(e.payload)),
  onHintDone: (cb: (e: HintDoneEvent) => void) => listen<HintDoneEvent>("hint-done", (e) => cb(e.payload)),
  onLevel: (cb: (level: number) => void) => listen<number>("recording-level", (e) => cb(e.payload)),
};
