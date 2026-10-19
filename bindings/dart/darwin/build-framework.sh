#!/usr/bin/env bash
#
# Repackage a built .dylib into a <name>.framework so that an iOS Archive passes App
# Store validation (loose .dylib files are rejected; a .framework bundle is accepted).
# The framework binary's install name becomes @rpath/<name>.framework/<name>, which is
# what the Dart side opens via DynamicLibrary.open (see
# bindings/dart/lib/src/server/bindings.dart for the OuisyncService case).
#
# Usage: build-framework.sh <in.dylib> <out-dir> <ios|macos> [name]
#   ios   -> flat framework (binary at bundle root)          -> iOS device slice
#   macos -> versioned framework (Versions/A/...)            -> macOS universal slice
#   name  -> defaults to OuisyncService
#
set -euo pipefail

DYLIB="$1"; OUT="$2"; PLATFORM="$3"; NAME="${4:-OuisyncService}"
FW="$OUT/$NAME.framework"

rm -rf "$FW"
mkdir -p "$FW"

if [ "$PLATFORM" = macos ]; then
  BIN="$FW/Versions/A/$NAME"
  mkdir -p "$FW/Versions/A/Resources"
  cp "$DYLIB" "$BIN"
  ln -sf A "$FW/Versions/Current"
  ln -sf "Versions/Current/$NAME" "$FW/$NAME"
  ln -sf Versions/Current/Resources "$FW/Resources"
  PLIST="$FW/Versions/A/Resources/Info.plist"
  MINOS_KEY=LSMinimumSystemVersion; MINOS=10.14
elif [ "$PLATFORM" = ios ]; then
  BIN="$FW/$NAME"
  cp "$DYLIB" "$BIN"
  PLIST="$FW/Info.plist"
  MINOS_KEY=MinimumOSVersion; MINOS=13.0
else
  echo "unknown platform: $PLATFORM (expected ios|macos)" >&2
  exit 1
fi

# Rewrite the install name so @rpath resolves to the embedded framework binary.
install_name_tool -id "@rpath/$NAME.framework/$NAME" "$BIN"

/usr/libexec/PlistBuddy \
  -c "Add :CFBundleIdentifier string net.ouisync.$NAME" \
  -c "Add :CFBundleName string $NAME" \
  -c "Add :CFBundleExecutable string $NAME" \
  -c "Add :CFBundlePackageType string FMWK" \
  -c "Add :CFBundleInfoDictionaryVersion string 6.0" \
  -c "Add :CFBundleVersion string 0.0.1" \
  -c "Add :CFBundleShortVersionString string 0.0.1" \
  -c "Add :$MINOS_KEY string $MINOS" \
  "$PLIST"

# install_name_tool invalidated any signature; add an ad-hoc one so the framework
# can be dlopen'd during local dev. CocoaPods re-signs it with the app's identity
# (incl. a Distribution profile at archive time) in the "Embed Pods Frameworks" phase.
codesign --force --sign - --timestamp=none "$FW" >/dev/null 2>&1 || \
  codesign --force --sign - "$BIN" >/dev/null 2>&1 || true

echo "Built $FW"
