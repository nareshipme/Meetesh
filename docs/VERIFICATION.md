# What has been verified by hand

Automated tests and CI say the code compiles and its logic works. This page tracks what a person has actually **tried on real hardware**, because that is what the tests cannot show. The steps for each row are in [TESTING.md](../TESTING.md).

How to read it:
- **Verified** = a person did it on the named OS/hardware and it worked. Add the date and who.
- **Failed** = tried and it did not work; link the issue.
- **Not yet** = only automated checks exist.

Rule: when someone reports a manual result, update this table in the same change (see the definition of done in [CONTRIBUTING.md](../CONTRIBUTING.md)). Never mark a row verified from CI alone.

| Area | What to check | Automated evidence | Manual status |
|---|---|---|---|
| App opens | `npm run tauri dev` starts and shows the UI | CI builds on Linux, macOS, Windows | **Verified** on macOS and Windows (maintainer) |
| Live assist hint quality | Hints are short, relevant, useful | Real Gemma 4 E2B test on scripted dialogue | **Verified** "good enough" on macOS/Windows (maintainer, first version) |
| Microphone recording, crash recovery, export | TESTING.md: Recording, Stopping, Crash recovery | e2e test with synthetic audio | Not yet (beyond the app opening) |
| Live transcription | Text appears a few seconds after speech | Fake and real-Whisper tests on synthetic speech | Not yet |
| Quiet and distant voices ([#4](https://github.com/nareshipme/Meetesh/issues/4)) | "Quiet and distant voices" in TESTING.md | WER bench on synthetic speech | Not yet |
| Model download, pause, resume, retry | TESTING.md: Model downloads | Local test server | Not yet |
| Model loading at startup, loading bar | TESTING.md: Model loading | Unit tests | Not yet |
| GPU on Mac ([#3](https://github.com/nareshipme/Meetesh/issues/3)) | `gpu_probe` output, row says GPU | Metal build compiles in CI | **Failed** on at least one Mac (Decode Error -3); waiting for `gpu_probe` output |
| GPU on Windows/Linux (Vulkan) | Row says GPU, hints faster | Software Vulkan device on Linux | Not yet |
| Hints do not slow live text | "Live assist" last items in TESTING.md | Measured with both real models on a 4-core CPU | Not yet |
| One-click recording | Ctrl/Cmd+N then Enter | Mock-backend UI checks | Not yet |
| **System audio, Windows** | TESTING.md: System audio | Compiles in CI; synthetic two-track tests | Not yet |
| **System audio, macOS** (permission prompt, ScreenCaptureKit) | TESTING.md: System audio | Compiles in CI | Not yet |
| Silence gap keeps real time (loopback) | Play nothing for 30 s, then speech: timestamp near 0:30 | `GapFiller` unit tests | Not yet |
| You / Others labels | Transcript shows both sides | Unit and UI mock tests | Not yet |
| **Meeting notes** | TESTING.md: Meeting notes | Real Gemma on scripted dialogue (29 s short call, 222 s for 26 min on 4-core CPU) | Not yet |
| **Auto titles** | TESTING.md: Automatic titles | Real Gemma title in 6.4 s | Not yet |
| **Auto hints** (questions, pauses) | TESTING.md: Automatic hints | Detector unit tests; mock UI | Not yet. Thresholds are untuned guesses |
| **Search** | TESTING.md: Search and export | 500 meetings / 200,000 lines: index 0.3 s, search 80 ms | Not yet |
| **Markdown export** | Copy / Export Markdown | Unit tests | Not yet |
| **Live screen / review page / Settings dialog** (the redesign) | TESTING.md: "Where things are" | Mock-backend UI checks (tabs, menu, dialog, narrow width) | Not yet |
| **Model choice per job** (speech, hints, notes) | TESTING.md: Choosing models | Selection rules unit-tested; real downloads and runs of the added models in a Linux sandbox | Not yet |
| Transcript follows the newest line | TESTING.md: Live transcription | Mock-backend checks | Not yet (the maintainer reported it not following in the real app; the fix is a hardening and could not be reproduced here) |
| "CPU (no GPU support in this build)" label on a normal Windows build | TESTING.md: GPU | Mock | Not yet; the maintainer saw plain "CPU" on a Windows machine with a GPU, which this label now explains |
| Readable default titles | New meeting is "Meeting on Sun 22 Nov, 3:45 PM" | Unit tests including the upgrade | Not yet |
| **Notes in the background**: line of jobs, paused (not cancelled) for hints and for a recording on a CPU | TESTING.md: Meeting notes | Unit tests of the rules; a real Gemma 3 1B run (hint 4 s into notes took 6.0 s, same as idle; notes finished) | Not yet |
| **Hints that remember earlier meetings** | TESTING.md: Hints that remember earlier meetings | Unit tests (keywords, related search, prompt); one scripted real-model case (generic hint became "revisit the $40,000 limit from the budget review") | Not yet |
| **Live points ("Now discussing")** | TESTING.md: Live points | Unit tests (parse, prompt, scheduler, gate); one scripted real-model case (16 to 20 s per update on a CPU) | Not yet. CPU cost on a slow machine is the open question |
| Packaged macOS build (macOS 13+, Info.plist microphone text) | Build a bundle and run it | Config only | Not yet |

## Log

Newest first. One line per report: date, who, what, result.

- 2026-11-22: maintainer on Windows: a machine with a GPU showed "CPU" for live assist (cause: a normal build contains no GPU backend; label added, see PR #19); the transcript did not seem to follow new lines; the interface felt too bulky (redesign); asked for per-job model choice, speaker recognition, hints that use earlier meetings, readable default titles.
- 2026-11-22: five features merged to `main` (#13 to #17). None of them has been run on real hardware yet.
