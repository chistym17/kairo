#!/usr/bin/env sh
# Install the latest Kairo binary from GitHub Releases (no Rust required).
#
#   curl -fsSL https://kairo.so/install.sh | sh
#
# Optional:
#   KAIRO_VERSION=v0.2.1   # install a specific release tag (default: latest)
#   KAIRO_INSTALL_DIR=~/.local/bin

set -eu

REPO="chistym17/agent-harness"
INSTALL_DIR="${KAIRO_INSTALL_DIR:-${HOME}/.local/bin}"
VERSION="${KAIRO_VERSION:-latest}"

say() { printf '%s\n' "$*"; }
err() { printf 'install.sh: %s\n' "$*" >&2; exit 1; }

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || err "missing required command: $1"
}

need_cmd curl
need_cmd tar
need_cmd mktemp
need_cmd uname

os="$(uname -s | tr '[:upper:]' '[:lower:]')"
arch="$(uname -m)"

case "$os" in
  linux) os_key="linux" ;;
  darwin) os_key="darwin" ;;
  *) err "unsupported OS: $(uname -s) (need Linux or macOS)" ;;
esac

case "$arch" in
  x86_64|amd64) arch_key="x86_64" ;;
  arm64|aarch64) arch_key="aarch64" ;;
  *) err "unsupported architecture: $arch" ;;
esac

case "${os_key}-${arch_key}" in
  linux-x86_64) target="x86_64-unknown-linux-gnu" ;;
  darwin-aarch64) target="aarch64-apple-darwin" ;;
  darwin-x86_64) target="x86_64-apple-darwin" ;;
  *)
    err "no prebuilt binary for ${os_key}/${arch_key} yet. Use: cargo install kairo"
    ;;
esac

asset="kairo-${target}.tar.gz"

if [ "$VERSION" = "latest" ]; then
  api_url="https://api.github.com/repos/${REPO}/releases/latest"
else
  # Accept both v0.2.1 and 0.2.1
  case "$VERSION" in
    v*) tag="$VERSION" ;;
    *) tag="v${VERSION}" ;;
  esac
  api_url="https://api.github.com/repos/${REPO}/releases/tags/${tag}"
fi

say "Fetching release metadata…"
release_json="$(curl -fsSL "$api_url")" || err "could not fetch release info from GitHub (tag/version may not exist yet)"

# Prefer a tagged download_url for our asset; fall back to constructed URL.
download_url="$(printf '%s' "$release_json" | sed -n "s/.*\"browser_download_url\": \"\\([^\"]*${asset}\\)\".*/\\1/p" | head -n 1)"
tag_name="$(printf '%s' "$release_json" | sed -n 's/.*"tag_name": "\([^"]*\)".*/\1/p' | head -n 1)"

if [ -z "$download_url" ]; then
  if [ -z "$tag_name" ]; then
    err "could not parse release tag from GitHub API"
  fi
  download_url="https://github.com/${REPO}/releases/download/${tag_name}/${asset}"
fi

tmpdir="$(mktemp -d)"
cleanup() { rm -rf "$tmpdir"; }
trap cleanup EXIT INT HUP

say "Downloading ${asset} (${tag_name:-$VERSION})…"
curl -fsSL "$download_url" -o "${tmpdir}/${asset}" || err "download failed: $download_url"

say "Extracting…"
tar -C "$tmpdir" -xzf "${tmpdir}/${asset}"
bin_path="$(find "$tmpdir" -type f -name kairo | head -n 1)"
[ -n "$bin_path" ] || err "archive did not contain a kairo binary"
chmod +x "$bin_path"

mkdir -p "$INSTALL_DIR"
install -m 0755 "$bin_path" "${INSTALL_DIR}/kairo"

say ""
say "Installed: ${INSTALL_DIR}/kairo"
if ! printf '%s' ":$PATH:" | grep -q ":${INSTALL_DIR}:"; then
  say ""
  say "Add this to your shell profile so \`kairo\` is found:"
  say "  export PATH=\"${INSTALL_DIR}:\$PATH\""
fi
say ""
say "Next:"
say "  kairo health"
say "  # first run prompts for provider + API key, or set DEEPSEEK_API_KEY / GROQ_API_KEY"
say "  cd /path/to/a/repo && kairo ask \"What does this project do?\""
