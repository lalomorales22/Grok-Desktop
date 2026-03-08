# Grok Desktop
<img width="2094" height="1140" alt="Screenshot 2026-03-08 at 11 58 17 AM" src="https://github.com/user-attachments/assets/88e3c94d-8acf-4736-b03b-8893e9facbd2" />

Grok Desktop is a macOS Tauri app for xAI with chat, workspace-aware tools, browser preview, a local terminal, media generation, a first-pass media editor, and an in-app IDE surface.

## Current Scope

- macOS only
- xAI-first workflow
- local-first storage for conversations, settings, workspace metadata, and imported media
- API key entered from the in-app Settings screen instead of a project `.env`

## Features

- Chat with Grok models and streamed responses
- Workspace indexing for prompt context
- Embedded terminal and browser preview
- `Imagine` page for image and video generation
- `Voice & Audio` page for TTS and realtime voice
- `Media Editor` page with timeline-style export flow
- `IDE` page for opening, editing, and saving indexed workspace files

## Install

The easiest way to use the app is the packaged macOS DMG from the repository's GitHub Releases page.

1. Download the latest `.dmg` from the repository's `Releases` page.
2. Open the DMG and move `Grok Desktop.app` into `Applications`.
3. Launch the app.
4. Open `Settings` inside the app and paste your xAI API key.

If macOS blocks the first launch because the app is unsigned, open it with `Right Click -> Open`.

## Build From Source

Prerequisites:

- macOS
- Node.js 20+
- Rust stable
- Xcode Command Line Tools
- `ffmpeg` if you want media export

Install dependencies:

```bash
npm install
```

Run in development:

```bash
npm run tauri dev
```

Create a production macOS build:

```bash
npm run tauri build
```

The generated DMG is written under `src-tauri/target/release/bundle/dmg/`.

## Local Data And Privacy

- No project `.env` file is required.
- Your xAI API key is saved locally by the app after you enter it in `Settings`.
- App data is stored in the platform app-data location for `GrokDesktop`.
- On macOS, the app now stores the API key in the system Keychain under the app service name.
- If an older plaintext key file exists under `~/Library/Application Support/GrokDesktop/secrets`, the app migrates it into Keychain automatically and removes the old file copy.
- The local SQLite database is also stored in the app-data directory, not in this repository.

That means cloning or publishing this repository does not include your API key or conversation database unless you manually copy local app-data files into the repo yourself.

## Validation

```bash
npm test
npm run build
cd src-tauri && cargo check
```

## Release Notes For Maintainers

- Do not commit `dist/`, `src-tauri/target/`, `.env*`, local database files, or macOS metadata files.
- Upload the built DMG to GitHub Releases instead of committing build artifacts to the repository.
- This repo currently targets macOS only.

## Project Layout

- `src/`: React UI, state store, tests, and Tauri bridge bindings
- `src-tauri/`: Rust commands, storage layer, native integration, workspace indexing, terminal, and editor export backend
- `src-tauri/icons/`: generated application icons

## Handoff

See [handoff.md](handoff.md) for the current development handoff and next-step plan.
