# Manual testing (Slices 0 to 3)

Run these on a real machine with a microphone. Automated tests cover the core logic; these cover what they
cannot: real audio hardware, OS permissions, and the UI.

## 1. Prerequisites
- Rust (`rustup`) and Node 20+
- A C/C++ toolchain, `cmake`, and clang/libclang (Whisper is compiled from source on the first build)
- Tauri system dependencies for your OS: https://tauri.app/start/prerequisites/
  - Ubuntu: `build-essential cmake clang libclang-dev libasound2-dev libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev libayatana-appindicator3-dev`
  - macOS: Xcode command line tools, `brew install cmake`
  - Windows: Visual Studio Build Tools (C++), CMake, LLVM

```bash
npm install
npm run tauri dev
```
The first build takes several minutes. Later runs are fast.

## 2. Automated tests first
```bash
cargo test --workspace
```
Optional, with a real model (downloaded by the app, or from the manifest URL):
```bash
MEETESH_TEST_MODEL=/path/to/ggml-tiny.en.bin \
  cargo test -p meetesh-core --features whisper --test whisper_fixture --test whisper_live -- --nocapture
```

## 3. Checklist

**Meetings**
- [ ] App opens with "No meetings yet." Click **+ New meeting**: it appears in the list and is selected.
- [ ] Edit the title and click elsewhere: the list updates. Restart the app: the title persisted.
- [ ] **Delete** asks for confirmation and removes the meeting.

**Speech model** (sidebar)
- [ ] Shows "Whisper Base (English)" with **Download** and the size (about 148 MB).
- [ ] Click Download: the progress bar moves. Click **Cancel** part-way, then Download again: it resumes (check that it does not restart from 0).
- [ ] When finished it says "Ready, runs on this device." The file is in the app data folder under `models/`.
- [ ] Turn off your network now. Everything below must still work offline.

**Recording**
- [ ] Select the meeting, click **Record**: a consent notice appears. Click "Everyone knows, start recording".
- [ ] The OS may ask for microphone permission. Allow it.
- [ ] The timer counts up and the level meter moves when you talk and falls when you are quiet.
- [ ] The microphone dropdown lists your devices. Choosing a non-default one records from it.
- [ ] The sidebar shows a red dot next to the recording meeting. **Delete** is disabled while recording.

**Live transcription** (model installed, while recording)
- [ ] The Transcript heading shows "● Live".
- [ ] Say a sentence, then pause for about a second: its text appears within a few seconds with a timestamp.
- [ ] Speak several sentences with pauses: segments appear in order with increasing timestamps.
- [ ] Talk continuously for 10+ seconds without pausing: text still appears (it is cut about every 6 seconds).
- [ ] Stay silent for 10 seconds: nothing is invented (no "thank you", no repeated phrases).

**Live assist** (needs the assist model: about 3.1 GB, in the sidebar)
- [ ] The sidebar lists two models. Download "Gemma 4 E2B (live assist)". The progress bar moves and the **✦ Hint** button stays disabled until it is ready.
- [ ] While recording (or on any meeting that has a transcript), click **✦ Hint** or press Ctrl/Cmd+Shift+H. The first hint loads the model (a few seconds), then text streams into a blue panel above the transcript.
- [ ] The hint is short (one or two sentences), relevant to what was just said, and shows how long it took. Dismiss it with ×.
- [ ] **Stop** while it is writing cancels it cleanly. Asking again right away works.
- [ ] Open "Meeting goals", type a goal, click elsewhere, and ask for a hint: it should take the goal into account. Restart the app and check the goal was saved.
- [ ] Ask for a hint while live transcription is running. Note whether the live text keeps up (a CPU-only machine may fall behind; that is expected and a gap-filling pass runs after stopping).
- [ ] On macOS, check that generation is fast (the app is built with Metal). Please report the time shown on the hint, your chip, and memory use in Activity Monitor.

**Stopping**
- [ ] Click **Stop**: it returns instantly. The meeting shows "Recorded m:ss".
- [ ] After a few seconds the transcript is complete (a gap-filling pass may run if live fell behind).
- [ ] **Export audio (WAV)** shows a path. Open that file and play it: it should be your full recording.
- [ ] Select another meeting and come back, then restart the app: the transcript is still there.

**Transcribe and re-transcribe**
- [ ] **Re-transcribe** shows a progress bar and then replaces the transcript. **Cancel** mid-way keeps the old transcript.
- [ ] Record a meeting before downloading the model: no live text, with a hint to download it. After downloading, **Transcribe** works on that recording.

**Crash recovery**
- [ ] Start recording, talk for 10+ seconds, then force-kill the app (`kill -9`, Task Manager, or Activity Monitor).
- [ ] Relaunch: the meeting says the recording was interrupted and the audio was kept. Export and Transcribe work on it. About 1 second at the end may be missing.

**Errors**
- [ ] Deny microphone permission (or unplug the mic) and click Record: you get a readable error and no empty "recording" meeting is left behind.

**Layout**
- [ ] Shrink the window to about 360 px wide: only the list or the meeting shows at a time, with a "← Meetings" button.
- [ ] Keyboard: Tab reaches every button and field with a visible focus ring. Try dark mode too.

**Longer run**
- [ ] Record 30+ minutes with live transcription. Watch memory in the OS monitor: it should stay flat. Afterwards search the log for `dropped` (see below): there should be no "dropped frames" warning.

## 4. Where things are
App data folder (database, recordings, models):
- Linux: `~/.local/share/app.meetesh.desktop/`
- macOS: `~/Library/Application Support/app.meetesh.desktop/`
- Windows: `%APPDATA%\app.meetesh.desktop\`

Logs are written by the Tauri log plugin to the app log folder (on Linux, `logs/Meetesh.log` inside the data folder; on macOS under `~/Library/Logs/app.meetesh.desktop/`). Logs never contain transcript text. Useful lines:
- `recording stopped for meeting N (... ms)`
- `recording N finished with X dropped frames`: should never appear. Please report it.
- `live transcription for meeting N: A chunks, B dropped`: B above 0 means the machine was too slow for live mode and a full pass ran.

## 5. Reporting problems
Note your OS, CPU, whether the model was tiny or base, and the relevant log lines (no audio or transcript needed).

## Known limits right now
Microphone only (no system audio), English models only, one recording per meeting, no summaries or live hints yet.
