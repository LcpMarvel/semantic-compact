#!/bin/sh
# Setup hook: provision bin/semantic-compact for this plugin copy.
# Order: an existing local build (target/release) wins — that is the dev
# loop — otherwise download the platform binary from the GitHub release
# matching the version in .claude-plugin/plugin.json, verifying its checksum.
# Always exits 0: a missing binary degrades to a silently failing
# UserPromptSubmit hook, never a broken session.
set -u

root="$(cd "$(dirname "$0")/.." && pwd)"
dest="$root/bin/semantic-compact"
releases_base="${SC_RELEASES_URL:-https://github.com/LcpMarvel/semantic-compact/releases/download}"

log() { printf 'semantic-compact setup: %s\n' "$1" >&2; }

if [ -x "$dest" ]; then
  exit 0
fi

# Local cargo build: the everyday development path.
if [ -x "$root/target/release/semantic-compact" ]; then
  mkdir -p "$root/bin"
  cp "$root/target/release/semantic-compact" "$dest" && exit 0
fi

# No toolchain requirement for end users: fetch the prebuilt binary.
version="$(sed -n 's/.*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$root/.claude-plugin/plugin.json" | head -n 1)"
if [ -z "$version" ]; then
  log "cannot read version from plugin.json"
  exit 0
fi

case "$(uname -s)/$(uname -m)" in
  Darwin/arm64)   triple="aarch64-apple-darwin" ;;
  Darwin/x86_64)  triple="x86_64-apple-darwin" ;;
  Linux/x86_64)   triple="x86_64-unknown-linux-gnu" ;;
  Linux/aarch64)  triple="aarch64-unknown-linux-gnu" ;;
  *) log "unsupported platform $(uname -s)/$(uname -m)"; exit 0 ;;
esac

base_url="$releases_base/v$version"
if ! command -v curl >/dev/null 2>&1; then
  log "curl not found, skipping binary download"
  exit 0
fi

tmpdir="$(mktemp -d 2>/dev/null || echo "/tmp/sc-setup-$$")"
mkdir -p "$tmpdir" "$root/bin"
trap 'rm -rf "$tmpdir"' EXIT

asset="semantic-compact-$triple"
if ! curl -fsSL "$base_url/$asset" -o "$tmpdir/$asset"; then
  log "download failed for $asset (v$version)"
  exit 0
fi

if ! curl -fsSL "$base_url/SHA256SUMS" -o "$tmpdir/SHA256SUMS"; then
  log "checksum list unavailable; skipping install"
  exit 0
fi
expected="$(awk -v f="$asset" '$2 == f { print $1; exit }' "$tmpdir/SHA256SUMS")"
if [ -z "$expected" ]; then
  log "checksum missing for $asset; skipping install"
  exit 0
fi
if command -v shasum >/dev/null 2>&1; then
  actual="$(shasum -a 256 "$tmpdir/$asset" | awk '{ print $1 }')"
elif command -v sha256sum >/dev/null 2>&1; then
  actual="$(sha256sum "$tmpdir/$asset" | awk '{ print $1 }')"
else
  log "no sha256 tool found; skipping install"
  exit 0
fi
if [ -z "$actual" ] || [ "$actual" != "$expected" ]; then
  log "checksum mismatch for $asset; skipping install"
  exit 0
fi

if chmod +x "$tmpdir/$asset" && mv "$tmpdir/$asset" "$dest"; then
  log "installed v$version ($triple)"
fi
exit 0
