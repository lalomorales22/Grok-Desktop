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
- `Hands` page for paired phone access through either the built-in `Hands Relay` provider or a Cloudflare tunnel fallback, with mobile chat and remote image/video/audio generation

## Hands

`Hands` is the mobile access surface for Grok Desktop.

- It starts a localhost-only bridge inside the app.
- It can expose that bridge through:
  - `Hands Relay`, a relay service you control
  - `Cloudflare tunnel`, as a fallback transport
- It requires a one-time pairing code from the desktop UI before the phone gets an authenticated session.
- It provides a mobile web interface for:
  - chat
  - image generation
  - video generation
  - audio generation
- It shows phone connections, incoming activity, and generated files on the desktop `Hands` page.
- It writes a dedicated local `hands-workspace` folder with activity logs and asset manifests.

Current requirements and limits:

- For `Hands Relay`, you need a running relay service and a configured relay URL.
- For `Cloudflare tunnel`, `cloudflared` must either be on your `PATH` or configured in the `Hands` page executable field.
- The mobile bridge is currently separate from the main desktop chat history.
- The first relay pass is request/response oriented and does not yet include richer live job streaming or shell/task execution over the relay.

## Hands Relay

This repository now includes a standalone relay service under [`hands-relay/`](hands-relay).

Run it locally:

```bash
cd hands-relay
npm install
npm run start
```

Local development default:

- Relay URL: `http://127.0.0.1:8787`
- Mobile page pattern: `http://127.0.0.1:8787/m/:machineId`

For real away-from-home access, deploy that relay on a public HTTPS host, then set the `Hands Relay URL` field inside the app to that origin.

The repository now includes a Render Blueprint at [`render.yaml`](/Users/megabrain2/Software/Rust-Apps/Grok-Desktop/render.yaml) for deploying `hands-relay` as a public HTTPS service.

## Deploy Hands Relay

The fastest production path is Render.

If you plan to share Grok Desktop with other users, each user should either:

- deploy their own `Hands Relay` service, or
- use a relay service you operate for them

The app does not magically create a public HTTPS endpoint from a local machine by itself. Off-site `Hands` access requires a public relay origin.

Before you deploy:

1. Commit and push the latest repository state to GitHub.
2. Make sure the repository root contains [`render.yaml`](/Users/megabrain2/Software/Rust-Apps/Grok-Desktop/render.yaml).
3. Make sure the relay service files are present under [`hands-relay/`](/Users/megabrain2/Software/Rust-Apps/Grok-Desktop/hands-relay).

Render setup:

1. Sign in to Render.
2. Click `New +`.
3. Choose `Blueprint`.
4. Connect the GitHub repository that contains this project.
5. Render should detect [`render.yaml`](/Users/megabrain2/Software/Rust-Apps/Grok-Desktop/render.yaml).
6. Review the `hands-relay` service that the Blueprint defines.
7. Deploy it.
8. After the service goes live, copy the Render HTTPS URL, for example:

```text
https://hands-relay.onrender.com
```

9. In Grok Desktop, open `Hands`.
10. Set `Provider` to `Hands Relay`.
11. Set `Hands Relay URL` to the Render HTTPS URL.
12. Click `Start secure link`.
13. Wait for the QR code and mobile URL to refresh.

## Relay Setup For Other Users

If someone else installs Grok Desktop and wants `Hands` remote access, they need to configure a public relay too.

Per-user setup flow:

1. Fork or clone this repository.
2. Deploy `hands-relay` to Render or another public HTTPS host.
3. Copy the deployed relay URL.
4. In Grok Desktop, open `Hands`.
5. Set `Provider` to `Hands Relay`.
6. Paste the deployed relay URL into `Hands Relay URL`.
7. Click `Start secure link`.
8. Scan the QR code from the phone and finish pairing with the one-time code.

If you operate a shared relay for multiple users:

- they can all point `Hands Relay URL` at the same deployed relay origin
- each desktop app generates its own machine identity and pairing flow
- each phone still pairs against a specific machine session

That means one public relay service can support multiple Grok Desktop users, but every user still has to configure the app to point at that relay.

Update behavior:

- changes to the mobile `Hands` web experience inside `hands-relay/` require a relay redeploy
- changes to the desktop app itself require rebuilding or reinstalling Grok Desktop

Optional production hardening:

- Attach a custom domain in Render later if you want your own hostname.
- Set `HANDS_PUBLIC_BASE_URL` in Render if you want to force the exact external origin.
- Keep in mind free-tier services may sleep and take time to wake up.

## Hands Startup Flow

Local relay development flow:

1. Start the relay:

```bash
cd hands-relay
npm install
npm run start
```

2. Open Grok Desktop and go to `Hands`.
3. Set `Provider` to `Hands Relay`.
4. Set `Hands Relay URL` to `http://127.0.0.1:8787`.
5. Click `Start secure link`.
6. When the desktop connects, `Hands` will show a public/mobile URL and generate a QR code.

For real off-site phone access:

1. Deploy `hands-relay` to a public HTTPS host.
2. Set `Hands Relay URL` in the app to that deployed origin, for example `https://hands.yourdomain.com`.
3. Start `Hands`.
4. Scan the QR code from the desktop app and complete pairing with the one-time code.

## Hands Troubleshooting

- `IO error: Connection refused (os error 61)`:
  - The app could not reach the configured `Hands Relay URL`.
  - Start `hands-relay`, or point the app at the correct deployed relay origin.

- `Tunnel unavailable. Install cloudflared or point Hands at the correct executable path.`:
  - This only applies when `Provider` is set to `Cloudflare tunnel`.
  - Switch to `Hands Relay` if you want to avoid the Cloudflare dependency.

- No QR code appears:
  - `Hands` only renders a QR code after it has a reachable public/mobile URL.
  - For local relay development, that means the relay must be running and the desktop must connect successfully.
  - For real remote use, the relay must be deployed on a public HTTPS host. A localhost relay URL will not work once you leave the network.

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
- `cloudflared` on your PATH if you want to use the Cloudflare fallback transport for `Hands`

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

Build outputs:

- DMG: `src-tauri/target/release/bundle/dmg/`
- App bundle: `src-tauri/target/release/bundle/macos/`

Install the locally built app bundle into `Applications`:

```bash
ditto "src-tauri/target/release/bundle/macos/Grok Desktop.app" "/Applications/Grok Desktop.app"
```

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
- `hands-relay/`: standalone public relay service for the `Hands` provider
- `src-tauri/icons/`: generated application icons

## Handoff

See [handoff.md](handoff.md) for the current development handoff and next-step plan.
