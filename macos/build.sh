#!/bin/zsh
# Builds Skykeep.app for macOS 26+ from this folder. Usage: ./build.sh [--dmg] [--install] [--run]
set -euo pipefail
cd "${0:A:h}"

# SwiftPM needs the full Xcode for the macOS SDK's SwiftUI; use it even if xcode-select points
# at the Command Line Tools.
if [[ -z "${DEVELOPER_DIR:-}" && -d /Applications/Xcode.app ]]; then export DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer; fi

# Universal (Apple silicon + Intel) when the toolchain can; this Mac's architecture otherwise.
if swift build -c release --arch arm64 --arch x86_64; then
  BIN="$(swift build -c release --arch arm64 --arch x86_64 --show-bin-path)/Skykeep"
else
  swift build -c release
  BIN="$(swift build -c release --show-bin-path)/Skykeep"
fi

APP="build/Skykeep.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/Skykeep"

# The iCloud helper (Rust, the same code as Skykeep for Windows and Linux): signs in to iCloud
# and reads and writes Notes over CloudKit. Built from the repository's workspace.
# Universal (Apple silicon + Intel) when the Intel target is installed
# (`rustup target add x86_64-apple-darwin`); this Mac's architecture otherwise.
if rustup target list --installed 2>/dev/null | grep -q x86_64-apple-darwin; then
  ( cd .. && cargo build --release -p skykeep-cloud --target aarch64-apple-darwin \
          && cargo build --release -p skykeep-cloud --target x86_64-apple-darwin )
  lipo -create -output "$APP/Contents/MacOS/skykeep-cloud" \
    ../target/aarch64-apple-darwin/release/skykeep-cloud ../target/x86_64-apple-darwin/release/skykeep-cloud
else
  ( cd .. && cargo build --release -p skykeep-cloud )
  cp ../target/release/skykeep-cloud "$APP/Contents/MacOS/skykeep-cloud"
fi
# Its own stable signature, so the keychain keeps trusting it across builds.
codesign --force --sign - --timestamp=none \
  -r='designated => identifier "io.github.matthfaria96.Skykeep.cloud"' \
  --identifier io.github.matthfaria96.Skykeep.cloud "$APP/Contents/MacOS/skykeep-cloud"
cp Resources/Info.plist "$APP/Contents/Info.plist"

# Icon: every size macOS asks for, from the 1024 master.
ICONSET="build/AppIcon.iconset"
if [[ ! -f build/AppIcon.icns || Resources/AppIcon-1024.png -nt build/AppIcon.icns ]]; then
  rm -rf "$ICONSET"; mkdir -p "$ICONSET"
  for s in 16 32 128 256 512; do
    sips -z $s $s Resources/AppIcon-1024.png --out "$ICONSET/icon_${s}x${s}.png" >/dev/null
    sips -z $((s*2)) $((s*2)) Resources/AppIcon-1024.png --out "$ICONSET/icon_${s}x${s}@2x.png" >/dev/null
  done
  iconutil -c icns "$ICONSET" -o build/AppIcon.icns
fi
cp build/AppIcon.icns "$APP/Contents/Resources/AppIcon.icns"

# Ad-hoc signature, enough to run on this Mac. Its designated requirement names the bundle
# identifier instead of this build's hash, so the permissions macOS grants (Calendar, Photos,
# Automation…) carry over to the next build instead of being asked again every time.
# For distribution, sign with a Developer ID and notarize (see README).
codesign --force --sign - --timestamp=none \
  -r='designated => identifier "io.github.matthfaria96.Skykeep"' "$APP"
echo "✓ $APP"

# --dmg: the download, as the other witchmymac apps ship it — the app beside a link to
# Applications, in build/Skykeep-<version>.dmg.
if [[ " $* " == *" --dmg "* ]]; then
  VERSION=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$APP/Contents/Info.plist")
  STAGE="build/dmg"
  rm -rf "$STAGE" "build/Skykeep-$VERSION.dmg"
  mkdir -p "$STAGE"
  ditto "$APP" "$STAGE/Skykeep.app"
  ln -s /Applications "$STAGE/Applications"
  hdiutil create -volname "Skykeep" -srcfolder "$STAGE" -ov -format UDZO "build/Skykeep-$VERSION.dmg" >/dev/null
  rm -rf "$STAGE"
  echo "✓ build/Skykeep-$VERSION.dmg"
fi

if [[ " $* " == *" --install "* ]]; then
  rm -rf /Applications/Skykeep.app
  cp -R "$APP" /Applications/
  echo "✓ instalado em /Applications/Skykeep.app"
  APP="/Applications/Skykeep.app"
fi
if [[ " $* " == *" --run "* ]]; then open "$APP"; fi
