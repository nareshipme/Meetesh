import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export interface Meeting {
  id: number;
  title: string;
  created_at: number; // unix seconds
  summary: string;
  transcript_model: string;
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
  onLevel: (cb: (level: number) => void) => listen<number>("recording-level", (e) => cb(e.payload)),
};
