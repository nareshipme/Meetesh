# Meetesh

A private, fully local meeting assistant: records, transcribes, and gives you live hints during the meeting. Nothing leaves your machine.

Status: **Slice 0 (walking skeleton)**. The app opens, talks to Rust, and stores meetings in SQLite. Recording, transcription, live assist and summaries come in the next slices (see the plan in Notion).

## Layout
```
crates/meetesh-core/   platform-neutral Rust core (storage now; models, assist logic later). No desktop-only deps.
src-tauri/             desktop shell: thin Tauri commands over the core
src/                   React + TypeScript frontend (all invoke() calls live in src/api.ts)
prototype-web/         the earlier Python/browser prototype, kept for reference
```

## Develop
Prerequisites: Rust, Node 20+, and the [Tauri system dependencies](https://tauri.app/start/prerequisites/) (on Ubuntu: `libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev libayatana-appindicator3-dev`).

```bash
npm install
npm run tauri dev
```

## Test
```bash
cargo test --workspace
```

## Build installers
```bash
npm run tauri build
```
Installers are produced for the OS you build on; CI builds all three.

## Rules of the road
- Business logic goes in `meetesh-core`, never in `src-tauri` (so a mobile shell can reuse it).
- Models are configuration (a manifest), not code. No model names hard-coded in logic.
- Never log transcript content.
- The app identifier `app.meetesh.desktop` in `src-tauri/tauri.conf.json` is hard to change after release: confirm it before shipping.
