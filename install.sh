#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-only
# Install the shrug binary on an NVIDIA DGX Spark.
#
#   curl -fsSL https://github.com/Enntity/shrug/releases/latest/download/install.sh | sh
#
# Options (environment):
#   SHRUG_VERSION      tag to install, e.g. v0.1.0 (default: latest)
#   SHRUG_INSTALL_DIR  install directory (default: $HOME/.local/bin)
#   SHRUG_BASE_URL     override the download base (testing only; default is
#                      the GitHub releases area for the chosen version)
set -eu

os=$(uname -s); arch=$(uname -m)
if [ "$os" != Linux ] || [ "$arch" != aarch64 ]; then
  echo "Shrug runs on NVIDIA DGX Spark (Linux aarch64)" >&2
  exit 1
fi

ver=${SHRUG_VERSION:-}
if [ -n "$ver" ]; then
  base="https://github.com/Enntity/shrug/releases/download/$ver"
else
  base="https://github.com/Enntity/shrug/releases/latest/download"
fi
base=${SHRUG_BASE_URL:-$base}

tarball=shrug-aarch64-unknown-linux-musl.tar.gz
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

fetch() {  # url dest
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$1" -o "$2"
  elif command -v wget >/dev/null 2>&1; then
    wget -q "$1" -O "$2"
  else
    echo "need curl or wget" >&2
    exit 1
  fi
}

echo "downloading $tarball from $base"
fetch "$base/$tarball" "$tmp/$tarball"
fetch "$base/SHA256SUMS" "$tmp/SHA256SUMS"

cd "$tmp"
grep " $tarball\$" SHA256SUMS | sha256sum -c - >/dev/null

tar -xzf "$tarball"
dir=${SHRUG_INSTALL_DIR:-$HOME/.local/bin}
mkdir -p "$dir"
install -m 0755 shrug "$dir/shrug"

"$dir/shrug" --version

case ":$PATH:" in
  *":$dir:"*) ;;
  *) echo "add to PATH: export PATH=\"$dir:\$PATH\"" ;;
esac

if ! command -v nvidia-smi >/dev/null 2>&1 || ! nvidia-smi --query-gpu=name --format=csv,noheader 2>/dev/null | grep -q GB10; then
  echo "warning: no GB10 GPU detected on this machine" >&2
fi

echo "Next: shrug   (shows what it found and what it would do; changes nothing)"
