# Manual testing

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
- [ ] App opens with "No meetings yet." Click **or a meeting for notes only**: an empty meeting appears in the list and is selected, with no consent notice.
- [ ] Edit the title and click elsewhere: the list updates. Restart the app: the title persisted.
- [ ] **Delete** asks for confirmation and removes the meeting.

**Speech model** (sidebar)
- [ ] Shows "Whisper Base (English)" with **Download** and the size (about 148 MB).
- [ ] Click Download: the progress bar moves. Click **Pause** part-way, then **Resume**: it continues (check that it does not restart from 0). More cases are under "Model downloads" below.
- [ ] When finished it says "ready" (with CPU). The file is in the app data folder under `models/`.
- [ ] Turn off your network now. Everything below must still work offline.

**Recording**
- [ ] Click **● New recording** (or press Ctrl/Cmd+N): a meeting is created and the consent notice is already open, with the "Everyone knows, start recording" button focused. Press Enter (or click it): recording starts. That is one click plus Enter, or two clicks.
- [ ] While recording, press Ctrl/Cmd+N or click New recording again: you are taken to the running recording with a message, and no second one starts.
- [ ] Click New recording, then **Cancel** or press Escape: the empty meeting disappears. If you renamed it first, it is kept.
- [ ] Choose a different microphone in the confirm step, quit and reopen the app: it is remembered (and falls back to the default if that device is no longer connected).
- [ ] On a notes-only meeting, **● Record** still works and shows the consent notice.
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

**Model downloads** (sidebar, bottom left)
- [ ] The sidebar lists two models (speech, live assist) with their sizes. Start the live-assist download (3.1 GB). The bar and "Downloading N% · x GB of y GB" move.
- [ ] Click **Pause**: it stops at once and says "Paused at N%". Click **Resume**: it continues from that point and does not start over.
- [ ] Quit the app while paused or mid-download and reopen it: the row shows "Paused at N%" with Resume and Discard.
- [ ] At 100% it shows "Verifying the download…" for a few seconds at most (not minutes), then the model starts loading by itself.
- [ ] Turn off the network mid-download: the row shows "Download failed at N%" with **Retry**. Turn the network on and Retry: it continues from where it stopped.
- [ ] **Discard** removes the partial download.

**Model loading** (automatic at start)
- [ ] Quit and reopen the app with both models installed. A bar at the top shows "Loading Gemma 4 E2B · N%" (a real percentage), then "Warming up", and disappears. The sidebar rows go from "Loading into memory N%" to "ready · GPU" (or "ready · CPU").
- [ ] While it is loading you can already create meetings and record. The **✦ Hint** button is greyed out with a tooltip until the model is ready.
- [ ] Right after a download finishes, the model loads without any click.
- [ ] Note how long loading and warm-up take, and which backend the row reports: "GPU", "CPU", or "CPU (GPU failed)". The last means the GPU path failed at startup; please send the log line "GPU failed while warming up ... (reason)".

**GPU** (needs the live-assist model downloaded)
- [ ] Note what the live-assist row says: "GPU", "CPU", or "CPU (GPU failed)". If it says "GPU failed", the sidebar shows the log folder: open `Meetesh.log` there and look for lines starting with `llama.cpp` (warnings and errors). Send them along.
- [ ] Run the GPU probe and send the whole output: `cargo run -p meetesh-core --example gpu_probe --features metal` (macOS), or `--features vulkan` (Windows/Linux, needs the Vulkan SDK).
- [ ] Windows/Linux with a GPU: run the app with `npm run tauri dev -- --features vulkan` and check the row says "GPU" and hints are faster than on the CPU. Report the time shown on a hint.

**Live assist** (needs the live-assist model)
- [ ] While recording (or on any meeting that has a transcript), click **✦ Hint** or press Ctrl/Cmd+Shift+H. The first hint is as quick as the later ones, because the model is already loaded. Text streams into a blue panel above the transcript.
- [ ] The hint is short (one or two sentences), relevant to what was just said, and shows how long it took and "GPU" or "CPU". Dismiss it with ×.
- [ ] If the GPU fails, the hint should still appear, labelled "CPU (fallback)". If you see an error such as "llama decode failed", please send the details (see Troubleshooting in the README).
- [ ] **Stop** while it is writing cancels it cleanly. Asking again right away works.
- [ ] Open "Meeting goals", type a goal, click elsewhere, and ask for a hint: it should take the goal into account. Restart the app and check the goal was saved.
- [ ] Ask for a hint repeatedly while you keep talking (live transcription running). The live text must keep up with your speech: it should stay a few seconds behind at most while hints are being generated. A hint may take a little longer to appear while you talk; that is expected, because it lets transcription go first. If live text still lags or the log says chunks were dropped, report it with your Mac model and whether the row says GPU or CPU.
- [ ] Please report the time shown on a hint, your chip, macOS version, and memory use in Activity Monitor.

**Meeting notes** (needs the live-assist model)
- [ ] Record a short meeting with at least a couple of sentences of real content (decisions and who-does-what make it easier to judge), then **Stop**. After the live transcript settles, a "Notes" section shows "Writing notes…" with a progress bar by itself, then a summary, decisions and action items.
- [ ] The notes are faithful: nothing invented (names, dates, numbers), decisions and action items match what was said. Note anything wrong or missing, with how long the meeting was and how long the notes took (the log has `summary for meeting N: … ms`).
- [ ] **Copy** puts Markdown on the clipboard. **Regenerate** writes new notes from the current transcript. **Remove** clears them (**✦ Write meeting notes** appears).
- [ ] **Cancel** during writing keeps any earlier notes. Starting a new recording while notes are being written cancels the notes (live transcription must not lag).
- [ ] A very short meeting (a few words) gets no automatic notes; the button still works if there is a transcript.
- [ ] Import a long recording (30+ minutes, or record one): notes still come out, in steps ("step 2 of 5"), and take minutes on a CPU, not hours. Report the time and whether the row says GPU or CPU.
- [ ] Without the live-assist model downloaded, the button is greyed with a tooltip, and no notes are written automatically.
- [ ] A call recorded with system audio: the notes can tell who said what ("you" versus the others).

**Automatic titles** (needs the live-assist model)
- [ ] Record a meeting without renaming it. When the notes appear, the title changes from "Meeting <date>" to a short title about what was said (also in the sidebar). Hovering the title says it was written by the model.
- [ ] Rename a meeting yourself, then **Regenerate** the notes: your title stays.
- [ ] Start typing a title while the notes are still being written: your text is not replaced when they finish.
- [ ] Upgrade from an older version with meetings you had renamed: their titles never change. A meeting still named "Meeting <date>" may get a title the next time its notes are written.

**Quiet and distant voices** (live transcription)
- [ ] Speak softly, then speak from about an arm's length or more away from the microphone. Live text should still appear. (Before this change nothing appeared when the level meter stayed in the bottom third.)
- [ ] Compare the live transcript with **Re-transcribe** on the same recording: they should be close. A big difference means the live path is losing words.
- [ ] If words are still missing, note what you said, how far away you were, which microphone, and whether the level meter moved. Export the audio if you are happy to share it.

**System audio: the other participants** (Windows and macOS 13+; this needs a real machine, it has not been run on real hardware yet)
- [ ] The consent step shows "Also record the other participants (sound this computer plays, such as a call)", ticked. On Linux it is replaced by a note that this is not available yet.
- [ ] **macOS:** click "Everyone knows". macOS asks for **Screen & System Audio Recording** permission for Meetesh (or for your terminal when running `npm run tauri dev`). Allow it, then quit and reopen the app (macOS only applies the permission after a restart) and record again.
- [ ] **Windows:** no prompt. Recording starts. If you have no speakers or headphones enabled you get a message and the microphone recording carries on.
- [ ] Wear headphones. Start a recording, play a YouTube video or join a call. A "+ Others" chip appears next to the timer.
- [ ] Within a few seconds the video's speech appears in the transcript labelled **Others**; you speaking appears as **You**. Timestamps of the two sides interleave sensibly.
- [ ] Stay silent and play nothing for 30 s, then play speech: the Others text appears with a timestamp near 0:30, not near 0:00 (the track keeps real time while nothing plays; Windows delivers no audio during silence, which is the case this checks).
- [ ] Untick the box, record again: no "+ Others" chip, no Others lines. Quit and reopen: the choice was remembered.
- [ ] macOS, permission denied (remove Meetesh in System Settings > Privacy & Security > Screen & System Audio Recording): you see "Could not capture system audio … Recording your microphone only." and the microphone recording works.
- [ ] Stop, then **Re-transcribe**: both sides come back, still labelled.
- [ ] Ask for a **✦ Hint** after the other side asked you a question: the hint should answer it (the model is told which lines are Others).
- [ ] Meetesh's own sounds are not recorded back (macOS excludes the app). Speakers instead of headphones: expect your voice to appear twice (You and Others); that is the known limit.
- [ ] Delete the meeting: the `system` folder in the recording folder is removed with it.

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
No system audio on Linux, English models only, one recording per meeting, no summaries yet, hints only on request.
