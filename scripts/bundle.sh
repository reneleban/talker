#!/bin/bash
# Baut ein installierbares talker.app (ad-hoc signiert, Personal Use).
# Nutzung: scripts/bundle.sh   →  target/bundle/talker.app
set -euo pipefail
cd "$(dirname "$0")/.."

echo "→ Release-Build …"
cargo build --release

echo "→ Icon generieren …"
cargo run --release --example gen_icon target/icon-1024.png

APP=target/bundle/talker.app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

echo "→ .icns bauen …"
ICONSET=target/talker.iconset
rm -rf "$ICONSET"
mkdir -p "$ICONSET"
for s in 16 32 128 256 512; do
  sips -z "$s" "$s" target/icon-1024.png --out "$ICONSET/icon_${s}x${s}.png" >/dev/null
  d=$((s * 2))
  sips -z "$d" "$d" target/icon-1024.png --out "$ICONSET/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/talker.icns"

echo "→ Bundle zusammensetzen …"
cp target/release/talker "$APP/Contents/MacOS/talker"
cp resources/Info.plist "$APP/Contents/Info.plist"

# sherpa-rs (download-binaries) linkt dynamisch gegen diese Dylibs; sie liegen
# unter target/release neben dem Binary. Ins Bundle einbetten + RPATH setzen,
# sonst startet die .app nicht (dyld: Library not loaded @rpath/...).
echo "→ Dylibs einbetten …"
mkdir -p "$APP/Contents/Frameworks"
cp target/release/libonnxruntime.1.17.1.dylib "$APP/Contents/Frameworks/"
cp target/release/libsherpa-onnx-c-api.dylib "$APP/Contents/Frameworks/"
install_name_tool -add_rpath "@executable_path/../Frameworks" "$APP/Contents/MacOS/talker"

# Stabile Signatur-Identität bevorzugen: bei Ad-hoc invalidiert jeder Rebuild
# die TCC-Permissions (Accessibility/Mikrofon müssten neu erteilt werden).
if security find-identity -p codesigning -v | grep -q '"talker-dev"'; then
  SIGN_ID="talker-dev"
else
  SIGN_ID="-"
  echo "⚠ kein talker-dev-Zertifikat — Ad-hoc-Signatur: Bedienungshilfen/Mikrofon"
  echo "  müssen nach JEDEM install neu erteilt werden. Fix (einmalig): make cert"
  sleep 5
fi
echo "→ Signatur ($SIGN_ID) …"
codesign --force -s "$SIGN_ID" "$APP/Contents/Frameworks/"*.dylib
codesign --force -s "$SIGN_ID" "$APP"

echo "✓ Fertig: $APP"
echo "  Installation (altes Bundle ERSETZEN, nie überkopieren — sonst"
echo "  killt macOS die App mit »Code Signature Invalid«):"
echo "    rm -rf ~/Applications/talker.app && cp -R $APP ~/Applications/"
