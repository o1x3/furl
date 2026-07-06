#!/bin/sh
# furl installer.
#
# Downloads the latest furl release for your platform from GitHub, verifies
# its checksum, and installs the furl, furls, and furl-manager binaries.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/o1x3/furl/main/install.sh | sh
#
# Environment:
#   PREFIX   Install prefix. Binaries go in $PREFIX/bin.
#            Defaults to /usr/local if writable, else $HOME/.local.
#   VERSION  Release tag to install (e.g. v0.1.0). Defaults to the latest.

set -eu

REPO="o1x3/furl"
BINS="furl furls furl-manager"

log() { printf '%s\n' "$*" >&2; }
err() { printf 'error: %s\n' "$*" >&2; exit 1; }

# --- pick a downloader -----------------------------------------------------
if command -v curl >/dev/null 2>&1; then
  DL="curl -fsSL"
  DL_O="curl -fsSL -o"
elif command -v wget >/dev/null 2>&1; then
  DL="wget -qO-"
  DL_O="wget -qO"
else
  err "neither curl nor wget found; please install one and retry"
fi

download()      { $DL "$1"; }              # to stdout
download_to()   { $DL_O "$2" "$1"; }       # url, dest

# --- detect os / arch ------------------------------------------------------
os="$(uname -s)"
arch="$(uname -m)"

case "$os" in
  Linux)  os_part="unknown-linux-gnu"; ext="tar.gz" ;;
  Darwin) os_part="apple-darwin";       ext="tar.gz" ;;
  MINGW*|MSYS*|CYGWIN*)
    os_part="pc-windows-msvc"; ext="zip"
    err "Windows detected; please download the .zip release manually from https://github.com/${REPO}/releases" ;;
  *) err "unsupported operating system: $os" ;;
esac

case "$arch" in
  x86_64|amd64) arch_part="x86_64" ;;
  aarch64|arm64) arch_part="aarch64" ;;
  *) err "unsupported architecture: $arch" ;;
esac

target="${arch_part}-${os_part}"

# --- resolve version -------------------------------------------------------
version="${VERSION:-}"
if [ -z "$version" ]; then
  log "Resolving latest release..."
  api="https://api.github.com/repos/${REPO}/releases/latest"
  # Extract the tag_name field without requiring jq.
  version="$(download "$api" | grep -m1 '"tag_name"' | sed -E 's/.*"tag_name"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1/')"
  [ -n "$version" ] || err "could not determine the latest release tag"
fi
log "Installing furl ${version} for ${target}"

# --- choose install prefix -------------------------------------------------
prefix="${PREFIX:-}"
if [ -z "$prefix" ]; then
  if [ -w /usr/local/bin ] 2>/dev/null || { [ -d /usr/local ] && [ -w /usr/local ]; }; then
    prefix="/usr/local"
  else
    prefix="$HOME/.local"
    log "No write access to /usr/local; installing to $prefix"
  fi
fi
bindir="${prefix}/bin"
mkdir -p "$bindir" || err "cannot create install directory: $bindir"

# --- download + verify -----------------------------------------------------
stem="furl-${version}-${target}"
asset="${stem}.${ext}"
base="https://github.com/${REPO}/releases/download/${version}"

tmp="$(mktemp -d 2>/dev/null || mktemp -d -t furl)"
trap 'rm -rf "$tmp"' EXIT INT TERM

log "Downloading ${asset}..."
download_to "${base}/${asset}"        "${tmp}/${asset}"        || err "failed to download ${asset}"
download_to "${base}/${asset}.sha256" "${tmp}/${asset}.sha256" || err "failed to download checksum"

log "Verifying checksum..."
( cd "$tmp"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum -c "${asset}.sha256"
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 -c "${asset}.sha256"
  else
    err "no sha256 tool (sha256sum/shasum) available to verify the download"
  fi
) || err "checksum verification failed"

# --- extract + install -----------------------------------------------------
log "Extracting..."
( cd "$tmp" && tar xzf "$asset" ) || err "failed to extract ${asset}"

for bin in $BINS; do
  src="${tmp}/${stem}/${bin}"
  [ -f "$src" ] || err "expected binary not found in archive: $bin"
  install -m 0755 "$src" "${bindir}/${bin}" 2>/dev/null \
    || { cp "$src" "${bindir}/${bin}" && chmod 0755 "${bindir}/${bin}"; } \
    || err "failed to install $bin to $bindir"
done

log ""
log "Installed: $BINS"
log "Location:  $bindir"
case ":${PATH}:" in
  *":${bindir}:"*) : ;;
  *) log ""; log "Note: ${bindir} is not on your PATH. Add it, e.g.:"
     log "  export PATH=\"${bindir}:\$PATH\"" ;;
esac
log ""
log "Run 'furl --version' to confirm."
