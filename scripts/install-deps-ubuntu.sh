#!/usr/bin/env bash
# Installs what building reconst-prep needs on Ubuntu (and other apt-based distros).
# CI runs this too, so a package added here is added to CI.
set -euo pipefail

usage() {
  cat <<'EOF'
usage: scripts/install-deps-ubuntu.sh [--with-ffmpeg]

Installs the system packages needed to build reconst-prep, then rustup and the
Rust toolchain pinned in rust-toolchain.toml. Safe to re-run: anything already
present is skipped.

  --with-ffmpeg  also install ffmpeg, which reconst-prep runs at runtime
                 (not needed to build or run the tests)
EOF
}

with_ffmpeg=0
for arg in "$@"; do
  case "$arg" in
    --with-ffmpeg) with_ffmpeg=1 ;;
    -h | --help) usage; exit 0 ;;
    *) echo "error: unknown argument: $arg" >&2; usage >&2; exit 2 ;;
  esac
done

if ! command -v apt-get >/dev/null; then
  echo "error: apt-get not found. On other distros install the equivalents of:" >&2
  echo "  a C compiler, pkg-config, curl, GTK 3 development headers (gtk+-3.0.pc)" >&2
  exit 1
fi

missing=()
need() {
  local check_desc=$1
  shift
  echo "missing: $check_desc"
  missing+=("$@")
}

# The C compiler links the final binary and builds ring.
command -v cc >/dev/null || need "C compiler (cc)" build-essential
command -v pkg-config >/dev/null || need "pkg-config" pkg-config
command -v curl >/dev/null || need "curl" curl ca-certificates
# rfd's file dialogs link GTK 3; this is the only native library linked at build time.
pkg-config --exists gtk+-3.0 2>/dev/null || need "GTK 3 headers (gtk+-3.0)" libgtk-3-dev
if [ "$with_ffmpeg" -eq 1 ]; then
  command -v ffmpeg >/dev/null || need "ffmpeg" ffmpeg
fi

if [ ${#missing[@]} -eq 0 ]; then
  echo "system packages: all present"
else
  sudo=()
  if [ "$(id -u)" -ne 0 ]; then
    if ! command -v sudo >/dev/null; then
      echo "error: not root and sudo is not installed; run as root or install: ${missing[*]}" >&2
      exit 1
    fi
    sudo=(sudo)
  fi
  "${sudo[@]}" apt-get update
  "${sudo[@]}" env DEBIAN_FRONTEND=noninteractive \
    apt-get install -y --no-install-recommends "${missing[@]}"
fi

if ! command -v rustup >/dev/null; then
  if [ "$(command -v cargo 2>/dev/null)" = /usr/bin/cargo ]; then
    echo "warning: cargo comes from the distro package, which may be too old for edition 2024; installing rustup alongside it." >&2
  fi
  echo "missing: rustup"
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain none
  export PATH="$HOME/.cargo/bin:$PATH"
  if [ -n "${GITHUB_PATH:-}" ]; then
    echo "$HOME/.cargo/bin" >>"$GITHUB_PATH"
  else
    echo "note: rustup added ~/.cargo/bin to your shell profile; open a new shell or run: . \"\$HOME/.cargo/env\""
  fi
fi
# With no argument, rustup installs the toolchain pinned in rust-toolchain.toml.
(cd "$(dirname "$0")/.." && rustup toolchain install)

if [ "$with_ffmpeg" -eq 0 ] && ! command -v ffmpeg >/dev/null; then
  echo "note: ffmpeg is not installed. reconst-prep needs it at runtime;"
  echo "      re-run with --with-ffmpeg or pass --ffmpeg-path to reconst-prep."
fi

echo "Dependencies installed."
