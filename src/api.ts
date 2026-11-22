import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export interface Meeting {
  id: number;
  title: string;
  created_at: number; // unix seconds
  /** Meeting notes as Markdown ("" until written). */
  summary: string;
  /** Who chose the title: the date placeholder, the model, or the user (never replaced automatically). */
  title_source: "default" | "model" | "user";
  /** Model that wrote the notes. */
  summary_model: string;
  transcript_model: string;
  /** Goals typed by the user; context for live hints. */
  agenda: string;
  status: "stopped" | "recording" | "recovered";
  duration_ms: number;
  audio_dir: string;
  /** The other participants (sound from this computer) were recorded as a second track. */
  has_system_audio: boolean;
}

export interface SearchMatch {
  kind: "title" | "notes" | "transcript";
  /** Text around the match; matched words sit between \u0001 and \u0002. */
  snippet: string;
  /** For transcript lines: when it was said. */
  start_ms: number | null;
}

export interface SearchResult {
  meeting_id: number;
  title: string;
  created_at: number;
  matches: SearchMatch[];
}

export interface InputDevice {
  name: string;
  is_default: boolean;
}

export interface TranscriptSegment {
  start_ms: number;
  end_ms: number;
  text: string;
  /** "mic" = you, "system" = the other participants. */
  source: "mic" | "system";
}

export interface SystemAudioSupport {
  available: boolean;
  /** Why not, in words for the user. Empty when available. */
  reason: string;
}

export interface StartedRecording {
  meeting: Meeting;
  /** Set when the other participants were asked for but could not be captured. */
  system_audio_error: string | null;
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
  /** Bytes on disk for a paused or failed download. */
  partial_bytes: number;
  is_default: boolean;
  /** Which jobs currently use this model: speech ("asr"), hints ("assist"), notes ("summary"). */
  selected_for: Array<"asr" | "assist" | "summary">;
}

export type ModelSlot = "asr" | "assist" | "summary";

/** The model chosen for each job. `summary_follows_hints`: notes use the same model as hints. */
export interface ModelSelection {
  asr: string;
  assist: string;
  summary: string;
  summary_follows_hints: boolean;
}

export type DownloadPhase = "downloading" | "verifying" | "paused" | "failed" | "done";

export interface DownloadEvent {
  id: string;
  phase: DownloadPhase;
  downloaded: number;
  total: number;
  error: string | null;
}

/** What the user chose for online meeting notes (the API key is never part of this). */
export interface OnlineSettings {
  enabled: boolean;
  auto: boolean;
  preset: string;
  base_url: string;
  auth_header: string;
  model: string;
}

export interface OnlinePreset {
  id: string;
  name: string;
  base_url: string;
  model: string;
  free_only: boolean;
  note: string;
}

export interface OnlineStatus extends OnlineSettings {
  has_key: boolean;
  keychain_error: string | null;
  presets: OnlinePreset[];
}

export type LoadState = "idle" | "loading" | "warming" | "ready" | "failed";

/** Where an installed model is on its way into memory. */
export interface ModelLoadEvent {
  id: string;
  role: "asr" | "assist";
  state: LoadState;
  /** 0..1 when the loader reports real progress; null = indeterminate. */
  progress: number | null;
  /** "GPU", "CPU", or "CPU (GPU failed)" when the GPU could not run the model. */
  backend: string | null;
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

export type NotesState = "idle" | "queued" | "running" | "paused";

export interface SummaryStatus {
  state: NotesState;
  /** Notes jobs ahead of this one (queued only). */
  position: number;
  /** Why it waits: a hint is being written, or you are recording. */
  paused_for: "hint" | "recording" | null;
}

export interface SummaryEvent {
  meeting_id: number;
  state: "queued" | "running" | "paused" | "finished";
  position: number;
  paused_for: "hint" | "recording" | null;
  done: number;
  total: number;
  /** The run ended (written, cancelled or failed); reload the meeting. */
  finished: boolean;
  cancelled: boolean;
  error: string | null;
}

/** Why a hint was offered without being asked; null for a hint the user requested. */
export type HintTrigger = "question" | "pause" | null;
export type AutoHintMode = "off" | "questions" | "questions_and_pauses";

/** The live list of what is being discussed. Also sent when the setting is switched (meeting_id may be 0). */
export interface LivePointsState {
  meeting_id: number;
  enabled: boolean;
  /** What is being discussed right now. */
  now: string;
  points: string[];
  /** An update is being written. */
  updating: boolean;
}

export interface HintPieceEvent {
  trigger: HintTrigger;
  meeting_id: number;
  hint_id: number;
  /** Everything generated so far (raw). */
  text: string;
}

/** An earlier meeting whose notes the model was shown for a hint. */
export interface HintSource {
  meeting_id: number;
  title: string;
  /** "Mon 16 Nov" */
  day: string;
}

export interface HintDoneEvent {
  sources: HintSource[];
  trigger: HintTrigger;
  meeting_id: number;
  hint_id: number;
  /** Cleaned hint; null when the model had nothing useful to say. */
  text: string | null;
  cancelled: boolean;
  error: string | null;
  first_token_ms: number;
  total_ms: number;
  tokens: number;
  on_gpu: boolean;
  /** The GPU failed and the CPU took over for this hint. */
  cpu_fallback: boolean;
}

// Thin typed wrapper over the Rust commands. Keep all invoke() calls here so the
// UI never depends on Tauri directly (easier to mock in tests, and for mobile).
export const api = {
  listMeetings: () => invoke<Meeting[]>("list_meetings"),
  createMeeting: (title?: string) => invoke<Meeting>("create_meeting", { title }),
  renameMeeting: (id: number, title: string) => invoke<Meeting>("rename_meeting", { id, title }),
  deleteMeeting: (id: number) => invoke<void>("delete_meeting", { id }),
  searchMeetings: (query: string) => invoke<SearchResult[]>("search_meetings", { query }),
  meetingMarkdown: (id: number) => invoke<string>("meeting_markdown", { id }),
  exportMarkdown: (id: number) => invoke<string>("export_markdown", { id }),
  listInputDevices: () => invoke<InputDevice[]>("list_input_devices"),
  activeRecording: () => invoke<number | null>("active_recording"),
  systemAudioSupport: () => invoke<SystemAudioSupport>("system_audio_support"),
  startRecording: (meetingId: number, device?: string, systemAudio?: boolean) =>
    invoke<StartedRecording>("start_recording", { meetingId, device, systemAudio }),
  stopRecording: () => invoke<Meeting>("stop_recording"),
  exportAudio: (id: number) => invoke<string>("export_audio", { id }),
  listModels: () => invoke<ModelInfo[]>("list_models"),
  modelSelection: () => invoke<ModelSelection>("model_selection"),
  /** `id` "" with slot "summary" means "same as hints". */
  selectModel: (slot: ModelSlot, id: string) => invoke<ModelSelection>("select_model", { slot, id }),
  downloadModel: (id: string) => invoke<void>("download_model", { id }),
  cancelDownload: () => invoke<void>("cancel_download"),
  deleteModel: (id: string) => invoke<void>("delete_model", { id }),
  discardDownload: (id: string) => invoke<void>("discard_download", { id }),
  modelRuntime: () => invoke<ModelLoadEvent[]>("model_runtime"),
  retryModelLoad: (id: string) => invoke<void>("retry_model_load", { id }),
  logDir: () => invoke<string | null>("log_dir"),
  onModelLoad: (cb: (e: ModelLoadEvent) => void) => listen<ModelLoadEvent>("model-load", (e) => cb(e.payload)),
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
  getLivePoints: () => invoke<LivePointsState>("get_live_points"),
  setLivePoints: (enabled: boolean) => invoke<void>("set_live_points", { enabled }),
  onLivePoints: (cb: (e: LivePointsState) => void) => listen<LivePointsState>("live-points", (e) => cb(e.payload)),
  getAutoHints: () => invoke<AutoHintMode>("get_auto_hints"),
  setAutoHints: (mode: AutoHintMode) => invoke<void>("set_auto_hints", { mode }),
  assistReady: () => invoke<boolean>("assist_ready"),
  activeHint: () => invoke<number | null>("active_hint"),
  onHintPiece: (cb: (e: HintPieceEvent) => void) => listen<HintPieceEvent>("hint-piece", (e) => cb(e.payload)),
  onHintDone: (cb: (e: HintDoneEvent) => void) => listen<HintDoneEvent>("hint-done", (e) => cb(e.payload)),
  summarizeMeeting: (id: number) => invoke<void>("summarize_meeting", { id }),
  /** Stop this meeting's notes, or take them out of the line. */
  cancelSummary: (id: number) => invoke<void>("cancel_summary", { id }),
  summaryStatus: (id: number) => invoke<SummaryStatus>("summary_status", { id }),
  clearSummary: (id: number) => invoke<Meeting>("clear_summary", { id }),
  onlineStatus: () => invoke<OnlineStatus>("online_status"),
  saveOnline: (settings: OnlineSettings, consent: boolean) => invoke<OnlineStatus>("save_online", { settings, consent }),
  setOnlineKey: (key: string) => invoke<OnlineStatus>("set_online_key", { key }),
  clearOnlineKey: () => invoke<OnlineStatus>("clear_online_key"),
  testOnline: (settings: OnlineSettings) => invoke<void>("test_online", { settings }),
  listOnlineModels: (settings: OnlineSettings) => invoke<string[]>("list_online_models", { settings }),
  onSummary: (cb: (e: SummaryEvent) => void) => listen<SummaryEvent>("summary-progress", (e) => cb(e.payload)),
  onLevel: (cb: (level: number) => void) => listen<number>("recording-level", (e) => cb(e.payload)),
};
