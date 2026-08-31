#!/usr/bin/env bash
set -euo pipefail

if pkg-config --exists gtk+-3.0 2>/dev/null; then
  exit 0
fi

if ! command -v apt-get >/dev/null; then
  echo "error: install the gtk 3 development package for your distribution" >&2
  exit 1
fi

sudo=""
if [ "$(id -u)" -ne 0 ]; then
  sudo=sudo
fi

$sudo apt-get update
$sudo apt-get install -y --no-install-recommends libgtk-3-dev
