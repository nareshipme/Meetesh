import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export interface Meeting {
  id: number;
  title: string;
  created_at: number; // unix seconds
  transcript: string;
  summary: string;
  status: "stopped" | "recording" | "recovered";
  duration_ms: number;
  audio_dir: string;
}

export interface InputDevice {
  name: string;
  is_default: boolean;
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
  onLevel: (cb: (level: number) => void) => listen<number>("recording-level", (e) => cb(e.payload)),
};
