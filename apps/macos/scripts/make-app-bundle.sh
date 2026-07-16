#!/usr/bin/env bash
#
# Wrap the SwiftPM-built executable into a minimal .app bundle so it can be
# launched from Finder. Full Xcode is NOT required: this uses only `swift build`
# and standard shell tools that ship with the macOS Command Line Tools.
#
# Usage:
#   apps/macos/scripts/make-app-bundle.sh [--configuration debug|release] [--output DIR]
#
set -euo pipefail

CONFIGURATION="release"
OUTPUT_DIR=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --configuration) CONFIGURATION="$2"; shift 2 ;;
    --output) OUTPUT_DIR="$2"; shift 2 ;;
    -h|--help)
      grep '^#' "$0" | sed 's/^# \{0,1\}//'
      exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

# Resolve the package directory (this script lives in apps/macos/scripts).
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PACKAGE_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
OUTPUT_DIR="${OUTPUT_DIR:-$PACKAGE_DIR/build}"

APP_NAME="Autophagy"
BUNDLE_ID="sh.autophagy.Autophagy"
EXECUTABLE="autophagy-app"

echo "Building ($CONFIGURATION) ..."
swift build --package-path "$PACKAGE_DIR" --configuration "$CONFIGURATION"

BIN_PATH="$(swift build --package-path "$PACKAGE_DIR" --configuration "$CONFIGURATION" --show-bin-path)"
BUILT_EXECUTABLE="$BIN_PATH/$EXECUTABLE"

if [[ ! -x "$BUILT_EXECUTABLE" ]]; then
  echo "error: built executable not found at $BUILT_EXECUTABLE" >&2
  exit 1
fi

APP_BUNDLE="$OUTPUT_DIR/$APP_NAME.app"
echo "Assembling $APP_BUNDLE ..."
rm -rf "$APP_BUNDLE"
mkdir -p "$APP_BUNDLE/Contents/MacOS"
mkdir -p "$APP_BUNDLE/Contents/Resources"

cp "$BUILT_EXECUTABLE" "$APP_BUNDLE/Contents/MacOS/$APP_NAME"

# The app is intentionally a normal Dock application by default: no LSUIElement
# key is written. The menu-bar extra is always present regardless, and the
# menu-bar-only (accessory, no Dock icon) mode is an opt-in runtime preference
# applied via NSApplication.setActivationPolicy — not a static Info.plist flag —
# so the default bundle keeps a Dock icon and a normal main window.
cat > "$APP_BUNDLE/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>$APP_NAME</string>
  <key>CFBundleDisplayName</key><string>$APP_NAME</string>
  <key>CFBundleIdentifier</key><string>$BUNDLE_ID</string>
  <key>CFBundleExecutable</key><string>$APP_NAME</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>CFBundleVersion</key><string>1</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST

echo "PkgInfo ..."
printf 'APPL????' > "$APP_BUNDLE/Contents/PkgInfo"

echo "Done: $APP_BUNDLE"
echo "Launch with: open \"$APP_BUNDLE\""
