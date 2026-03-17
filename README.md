# Grok Desktop
<img width="2094" height="1140" alt="Screenshot 2026-03-08 at 11 58 17 AM" src="https://github.com/user-attachments/assets/88e3c94d-8acf-4736-b03b-8893e9facbd2" />

Grok Desktop is a macOS Tauri app for xAI with a multi-terminal grid (Tiles), chat, workspace-aware tools, browser preview, a local terminal with full ANSI color support, media generation, a first-pass media editor, and an in-app IDE surface.

## Current Scope

- macOS only
- xAI-first workflow
- local-first storage for conversations, settings, workspace metadata, and imported media
- API key entered from the in-app Settings screen instead of a project `.env`

## Features

- `Tiles` page for opening a grid of independent terminal windows (1x2, 2x2, or 3x3 layouts) — each tile is a full PTY session with ANSI color support, perfect for running multiple Claude instances or any terminal workflow side-by-side
- Chat with Grok models and streamed responses
- Workspace indexing for prompt context
- Embedded terminal and browser preview with full ANSI color and truecolor support
- `Imagine` page for image and video generation
- `Voice & Audio` page for TTS and realtime voice
- `Media Editor` page with timeline-style export flow
- `IDE` page for opening, editing, and saving indexed workspace files
- `Hands` page for paired phone access through either the built-in `Hands Relay` provider or a Cloudflare tunnel fallback, with mobile chat and remote image/video/audio generation

## Hands

`Hands` is the mobile access surface for Grok Desktop.

- It starts a localhost-only bridge inside the app.
- It can expose that bridge through:
  - `Hands Relay`, a relay service **you deploy and control**
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

- For `Hands Relay`, you need to deploy your own relay service and configure its URL in the app.
- For `Cloudflare tunnel`, `cloudflared` must either be on your `PATH` or configured in the `Hands` page executable field.
- The mobile bridge is currently separate from the main desktop chat history.
- The first relay pass is request/response oriented and does not yet include richer live job streaming or shell/task execution over the relay.

## Hands Relay Security

> **Every message, prompt, and generated file you send through Hands passes through the relay server in plaintext.** Whoever operates the relay can see all of that data. For this reason:
>
> - **Always deploy your own relay.** Never paste someone else's relay URL into the app unless you fully trust them with all your Hands traffic.
> - **The app ships with no default relay URL.** You must deploy one yourself before Hands will work.
> - **The relay code is open source** and included in this repository under [`hands-relay/`](hands-relay) so you can audit exactly what it does.

## Deploy Your Own Hands Relay

Each user must deploy their own relay. The fastest path is Render (free tier works).

### Step 1 — Deploy to Render

1. Fork or clone this repository to your own GitHub account.
2. Sign in to [Render](https://render.com).
3. Click `New +` and choose `Blueprint`.
4. Connect the GitHub repository that contains your fork/clone.
5. Render will detect [`render.yaml`](render.yaml) and show a `hands-relay` service.
6. Deploy it.
7. After the service goes live, copy your Render HTTPS URL, for example:

```text
https://your-hands-relay.onrender.com
```

### Step 2 — Configure the App

1. Open Grok Desktop and go to **Hands**.
2. Set **Provider** to `Hands Relay`.
3. Paste your Render HTTPS URL into **Your Relay URL**.
4. Click **Save setup**, then **Start secure link**.
5. Scan the QR code from your phone and enter the one-time pairing code.

That's it. Your phone now connects to your desktop through your own relay.

### Local Development (Optional)

If you want to test the relay locally (phone and desktop on the same network):

```bash
cd hands-relay
npm install
npm run start
```

Then set `Your Relay URL` to `http://127.0.0.1:8787` in the app. This only works on the same Wi-Fi — for real remote access you need a public HTTPS deployment.

### Optional Production Hardening

- Attach a custom domain in Render if you want your own hostname.
- Set `HANDS_PUBLIC_BASE_URL` in Render environment variables to force the exact external origin.
- Free-tier Render services may sleep after inactivity and take a few seconds to wake up.

## Hands Troubleshooting

- `Hands relay URL is missing`:
  - You need to deploy your own relay and paste the URL into the Hands settings. See the deploy steps above.

- `IO error: Connection refused (os error 61)`:
  - The app could not reach the configured relay URL.
  - Make sure your relay is deployed and running, and the URL is correct.

- `Tunnel unavailable. Install cloudflared or point Hands at the correct executable path.`:
  - This only applies when `Provider` is set to `Cloudflare tunnel`.
  - Switch to `Hands Relay` if you want to avoid the Cloudflare dependency.

- No QR code appears:
  - `Hands` only renders a QR code after it has a reachable public/mobile URL.
  - Make sure your relay is deployed on a public HTTPS host and the app connected successfully.

## Install

### One-Line Install (Build From Source)

Clone the repo and run the install script. It checks for prerequisites (Xcode CLI Tools, Homebrew, Node.js 20+, Rust, ffmpeg), installs anything missing, builds the app, and copies it into `/Applications`:

```bash
git clone https://github.com/megabrain2/Grok-Desktop.git
cd Grok-Desktop
./install.sh
```

That's it. After the build finishes, launch **Grok Desktop** from Applications or Spotlight, open **Settings**, and paste your xAI API key.

### From a Release DMG

1. Download the latest `.dmg` from the repository's [Releases](https://github.com/lalomorales22/Grok-Desktop/releases) page.
2. Open the DMG and drag `Grok Desktop.app` into `Applications`.
3. **Important — the app is not code-signed yet.** macOS will block it on first launch. To open it:
   - Open **Terminal** and run:
     ```bash
     xattr -cr "/Applications/Grok Desktop.app"
     ```
   - Then **Right-click** (or Control-click) `Grok Desktop` in Applications and choose **Open**.
   - A dialog will warn that the app is from an unidentified developer. Click **Open**.
   - After this one-time step, the app opens normally with a regular double-click.
4. Open **Settings** inside the app and paste your xAI API key.

> **Why is this needed?** macOS Gatekeeper quarantines apps that are not signed with an Apple Developer certificate. The `xattr -cr` command removes that quarantine flag. This is standard for any open-source app distributed outside the Mac App Store without code signing.

## Build From Source (Manual)

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
