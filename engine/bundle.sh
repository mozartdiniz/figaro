#!/usr/bin/env bash
# Builds engine/bundle/: castlabs Electron (AAC + licensed Widevine) with
# the engine app in resources/app, and the binary renamed figaro-engine.
# The Electron zip is downloaded once into engine/.cache/ and checked
# against the release's SHA-256.
set -euo pipefail

VERSION="v44.5.1+wvcus"
SHA256="afa7d4ea49abf1e91c5be0f88cfe1abdb977ff3454d57658e90b004aacbde9cc"

here="$(cd "$(dirname "$0")" && pwd)"
out="$here/bundle"
zip="$here/.cache/electron-$VERSION-linux-x64.zip"
url="https://github.com/castlabs/electron-releases/releases/download/${VERSION/+/%2B}/electron-${VERSION/+/%2B}-linux-x64.zip"

if [[ "$(uname -s)" != Linux ]]; then
    echo "the playback engine is bundled on Linux only" >&2
    exit 2
fi

mkdir -p "$here/.cache"
if [[ ! -f "$zip" ]]; then
    echo "downloading castlabs Electron $VERSION…"
    curl -fL --progress-bar -o "$zip.part" "$url"
    mv "$zip.part" "$zip"
fi
echo "$SHA256  $zip" | sha256sum -c --quiet

rm -rf "$out"
mkdir -p "$out"
unzip -q "$zip" -d "$out"
mv "$out/electron" "$out/figaro-engine"
rm -f "$out/resources/default_app.asar"
mkdir -p "$out/resources/app/web"
cp "$here/package.json" "$here/main.js" "$out/resources/app/"
cp "$here/web/player.html" "$out/resources/app/web/"
echo "engine bundled at $out/figaro-engine"
