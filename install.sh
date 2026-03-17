#!/usr/bin/env bash
set -euo pipefail

# ──────────────────────────────────────────────
# Grok Desktop — macOS install script
# Installs prerequisites, builds the app, and
# copies the .app bundle into /Applications.
# ──────────────────────────────────────────────

APP_NAME="Grok Desktop"
BUNDLE_DIR="src-tauri/target/release/bundle/macos"
INSTALL_DIR="/Applications"

info()  { printf '\033[1;36m▸ %s\033[0m\n' "$*"; }
ok()    { printf '\033[1;32m✔ %s\033[0m\n' "$*"; }
warn()  { printf '\033[1;33m⚠ %s\033[0m\n' "$*"; }
fail()  { printf '\033[1;31m✖ %s\033[0m\n' "$*"; exit 1; }

# ── Platform check ──────────────────────────
[[ "$(uname)" == "Darwin" ]] || fail "Grok Desktop currently supports macOS only."

# ── Xcode Command Line Tools ────────────────
if ! xcode-select -p &>/dev/null; then
  info "Installing Xcode Command Line Tools…"
  xcode-select --install
  echo "    Re-run this script after the Xcode tools finish installing."
  exit 0
else
  ok "Xcode Command Line Tools found"
fi

# ── Homebrew ────────────────────────────────
if ! command -v brew &>/dev/null; then
  info "Installing Homebrew…"
  /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
  # Make brew available in this session
  eval "$(/opt/homebrew/bin/brew shellenv 2>/dev/null || /usr/local/bin/brew shellenv 2>/dev/null)"
  ok "Homebrew installed"
else
  ok "Homebrew found"
fi

# ── Node.js ─────────────────────────────────
if ! command -v node &>/dev/null; then
  info "Installing Node.js via Homebrew…"
  brew install node
  ok "Node.js installed ($(node -v))"
else
  NODE_MAJOR="$(node -v | sed 's/v//' | cut -d. -f1)"
  if (( NODE_MAJOR < 20 )); then
    warn "Node.js $(node -v) found but 20+ is required — upgrading…"
    brew upgrade node
  fi
  ok "Node.js $(node -v) found"
fi

# ── Rust ────────────────────────────────────
if ! command -v rustc &>/dev/null; then
  info "Installing Rust via rustup…"
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
  source "$HOME/.cargo/env"
  ok "Rust installed ($(rustc --version))"
else
  ok "Rust found ($(rustc --version))"
fi

# ── Optional: ffmpeg ────────────────────────
if ! command -v ffmpeg &>/dev/null; then
  info "Installing ffmpeg (needed for media export)…"
  brew install ffmpeg
  ok "ffmpeg installed"
else
  ok "ffmpeg found"
fi

# ── npm install ─────────────────────────────
info "Installing npm dependencies…"
npm install
ok "npm dependencies installed"

# ── Build ───────────────────────────────────
info "Building Grok Desktop (this may take a few minutes on first run)…"
npm run tauri build
ok "Build complete"

# ── Install to /Applications ────────────────
if [[ -d "$INSTALL_DIR/$APP_NAME.app" ]]; then
  warn "Removing existing $APP_NAME from $INSTALL_DIR…"
  rm -rf "$INSTALL_DIR/$APP_NAME.app"
fi

info "Copying $APP_NAME.app to $INSTALL_DIR…"
ditto "$BUNDLE_DIR/$APP_NAME.app" "$INSTALL_DIR/$APP_NAME.app"
ok "$APP_NAME installed to $INSTALL_DIR"

echo ""
ok "Done! Launch \"$APP_NAME\" from Applications or Spotlight."
echo "   Open Settings inside the app and paste your xAI API key to get started."
