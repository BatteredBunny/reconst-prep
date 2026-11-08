#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"

# --cli builds the command line alone: no `gui` subcommand and no windowing libraries to bundle.
NAME=reconst-prep
DESKTOP=reconst-prep.desktop
CARGO_FLAGS=()
if [ "${1:-}" = "--cli" ]; then
  # An underscore, not a hyphen: released GUI AppImages update from "reconst-prep-*", which a
  # "reconst-prep-cli-..." asset would match, so they would replace themselves with this one.
  NAME=reconst-prep_cli
  DESKTOP=reconst-prep-cli.desktop
  CARGO_FLAGS=(--no-default-features)
fi

VERSION=$(grep -m1 '^version' ../../Cargo.toml | cut -d'"' -f2)
ARCH=$(uname -m)
UPDATE_INFO="gh-releases-zsync|BatteredBunny|reconst-prep|latest|${NAME}-*-${ARCH}.AppImage.zsync"

( cd ../.. && cargo build --locked --release -p reconst-prep "${CARGO_FLAGS[@]}" )

APPDIR="AppDir-${NAME}"
rm -rf "$APPDIR"
mkdir -p "$APPDIR"/usr/bin "$APPDIR"/usr/share/applications "$APPDIR"/usr/share/icons/hicolor/scalable/apps
install -m755 ../../target/release/reconst-prep "$APPDIR"/usr/bin/
install -m644 "$DESKTOP" "$APPDIR"/usr/share/applications/
install -m644 ../../assets/icon.svg "$APPDIR"/usr/share/icons/hicolor/scalable/apps/reconst-prep.svg

LINUXDEPLOY=${LINUXDEPLOY:-}
if [ -z "$LINUXDEPLOY" ]; then
  if command -v linuxdeploy >/dev/null; then
    LINUXDEPLOY=linuxdeploy
  else
    curl -fsSL -o linuxdeploy "https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-${ARCH}.AppImage"
    chmod +x linuxdeploy
    LINUXDEPLOY=./linuxdeploy
  fi
fi

LDAI_UPDATE_INFORMATION="$UPDATE_INFO" \
LDAI_OUTPUT="${NAME}-${VERSION}-${ARCH}.AppImage" \
"$LINUXDEPLOY" --appdir "$APPDIR" \
  --desktop-file "$APPDIR/usr/share/applications/$DESKTOP" \
  --icon-file "$APPDIR"/usr/share/icons/hicolor/scalable/apps/reconst-prep.svg \
  --output appimage

# linuxdeploy can exit 0 when its appimage plugin fails, so check for the file itself.
if [ ! -f "${NAME}-${VERSION}-${ARCH}.AppImage" ]; then
  echo "linuxdeploy did not produce ${NAME}-${VERSION}-${ARCH}.AppImage" >&2
  exit 1
fi
echo "built: ${NAME}-${VERSION}-${ARCH}.AppImage"
