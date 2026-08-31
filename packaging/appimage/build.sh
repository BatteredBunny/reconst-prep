#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"

VERSION=$(grep -m1 '^version' ../../Cargo.toml | cut -d'"' -f2)
ARCH=$(uname -m)
UPDATE_INFO="gh-releases-zsync|BatteredBunny|reconst-prep|latest|reconst-prep-*-${ARCH}.AppImage.zsync"

( cd ../.. && cargo build --locked --release -p reconst-prep )

rm -rf AppDir
mkdir -p AppDir/usr/bin AppDir/usr/share/applications AppDir/usr/share/icons/hicolor/scalable/apps
install -m755 ../../target/release/reconst-prep AppDir/usr/bin/
install -m644 reconst-prep.desktop AppDir/usr/share/applications/
install -m644 ../../assets/icon.svg AppDir/usr/share/icons/hicolor/scalable/apps/reconst-prep.svg

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
LDAI_OUTPUT="reconst-prep-${VERSION}-${ARCH}.AppImage" \
"$LINUXDEPLOY" --appdir AppDir \
  --desktop-file AppDir/usr/share/applications/reconst-prep.desktop \
  --icon-file AppDir/usr/share/icons/hicolor/scalable/apps/reconst-prep.svg \
  --output appimage

echo "built: reconst-prep-${VERSION}-${ARCH}.AppImage"
