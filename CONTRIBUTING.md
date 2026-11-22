# Contributing to Meetesh

Thanks for helping. This is an early project, so small, focused changes are easiest to review.

## Getting set up
Follow "Quick start" in the [README](README.md), then run the tests:

```bash
cargo test --workspace
```

## Working on the UI without the desktop shell
The frontend can run in a plain browser against a fake backend (no Rust build, microphone or model needed):

```bash
npm install && npm run dev
```
Then open http://localhost:1420/?mock (or `?mock=empty`, `?mock=nomodel`). The fake backend lives in `src/devMock.ts`, is only loaded in development, and is not part of production builds.

## Principles
These keep the project private, portable and easy to change:

1. **Local first.** No feature may send audio or transcripts off the device by default. Anything that uses the network must be opt-in and clearly labelled.
2. **Platform-neutral core.** Business logic goes in `crates/meetesh-core`, not in `src-tauri`. The core must not depend on windowing, Tauri or OS-specific audio, so a mobile or other shell can reuse it.
3. **Models are configuration.** Do not hard-code model names in logic. Use the manifest and the traits (`Transcriber`, and later the assistant and summarizer).
4. **Never block the audio callback.** Capture must not wait on inference, disk or locks. Slow work goes behind a bounded queue.
5. **Never log transcript or audio content.** Log ids, counts and timings only.
6. **Measure audio changes.** Anything that changes voice detection, chunking or volume handling must come with before/after numbers from the transcription quality bench (see the README, "Transcription quality bench"). Keep what lowers the error rate on more than one model, drop what does not.
7. **Keep all `invoke()` calls in `src/api.ts`** so the UI stays easy to test and port.

## Definition of done
A piece of work (a slice, a feature, a notable fix) is not done until all of these are true:

1. **The README is up to date.** What works today, limitations, roadmap, troubleshooting, quick start and commands all match what the code now does. A stale README is a bug.
2. **`TESTING.md` covers the change**, so someone can check it by hand.
3. **Tests pass** (`cargo test --workspace`, `npm run build`) and new behavior has tests.
4. **The pull request says what was not tested** (for example real hardware, or another OS).

The pull request template repeats this as a checklist.

## Making a change
1. Open an issue for anything bigger than a small fix, so we can agree on the approach.
2. Branch from `main`, keep the change small, and add tests for new behavior. The core is designed to be testable without hardware (see the fake sources and transcribers in the existing tests).
3. Run `cargo test --workspace` and `npm run build` before opening a pull request.
4. In the pull request, say what you changed, why, and how you tested it. Say clearly what you could **not** test (for example real microphone capture on your OS).

## Reporting bugs
Include your OS, CPU, the model used (tiny or base), what you expected, and what happened. Relevant log lines help (the logs contain no transcript text). Please do not attach recordings of other people.

## License
By contributing you agree that your contribution is licensed under the [MIT License](LICENSE).
