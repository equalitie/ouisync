#!/usr/bin/env bash
# Build OuisyncExample and launch it as a proper macOS .app bundle so that it
# receives keyboard focus and appears in the Dock.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

swift build

BINARY=".build/debug/OuisyncExample"
APP=".build/OuisyncExample.app"

mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BINARY"     "$APP/Contents/MacOS/OuisyncExample"
cp "Info.plist"  "$APP/Contents/"

open "$APP"
