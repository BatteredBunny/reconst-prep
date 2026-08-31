#!/usr/bin/env bash
# Has to patch gyroflow-core to not download the lens profile at build time
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REV=$(sed -n 's/^gyroflow-core = .*rev = "\([0-9a-f]\{40\}\)".*/\1/p' "$root/Cargo.toml")
if [ -z "$REV" ]; then
  echo "error: could not read the gyroflow-core rev from $root/Cargo.toml" >&2
  exit 1
fi

glob="${CARGO_HOME:-$HOME/.cargo}/git/checkouts/gyroflow-*/${REV:0:7}*"

shopt -s nullglob

checkouts=()
find_checkouts() {
  checkouts=()
  for d in $glob; do
    if [ -d "$d" ]; then
      checkouts+=("$d")
    fi
  done
}

find_checkouts
if [ ${#checkouts[@]} -eq 0 ]; then
  cargo fetch --locked
  find_checkouts
fi

if [ ${#checkouts[@]} -eq 0 ]; then
  echo "error: no gyroflow checkout found for rev $REV, did cargo fetch fail?" >&2
  exit 1
fi

for d in "${checkouts[@]}"; do
  db="$d/resources/camera_presets/profiles.cbor.gz"
  [ -e "$db" ] && continue
  mkdir -p "$(dirname "$db")"
  : > "$db"
  echo "created gyroflow-core lens-profile placeholder: $db"
done
