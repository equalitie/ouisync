#!/usr/bin/env bash
# Build Logtee.framework (macOS + iOS device) for the Flutter `logtee` plugin.
# Run this once before `pod install`/`flutter build macos|ios`, and re-run whenever
# the logtee Rust crate changes. Modeled on
# bindings/swift/OuisyncLib/build-xcframework.sh's dylib-packaging step, but logtee
# has no Swift consumer, so there's no xcframework/static-lib side to build here -
# only the dynamic library the Dart FFI bindings dlopen.
#
# Prerequisites: cargo, rustup (targets: aarch64-apple-darwin x86_64-apple-darwin
# aarch64-apple-ios)
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../../" && pwd)" # .../ouisync
BUILD_DIR="$PROJECT_ROOT/target"
MK_FRAMEWORK="$PROJECT_ROOT/bindings/dart/darwin/build-framework.sh"

if [[ "${DEBUG:-}" == "1" ]]; then
  CONFIGURATION="debug"
  RELEASE_FLAG=""
else
  CONFIGURATION="release"
  RELEASE_FLAG="--release"
fi

# Match the Xcode project's MACOSX_DEPLOYMENT_TARGET (see build-xcframework.sh).
export MACOSX_DEPLOYMENT_TARGET=13.0

cd "$PROJECT_ROOT"
for TARGET in aarch64-apple-darwin x86_64-apple-darwin aarch64-apple-ios; do
  echo "==> Building logtee for $TARGET..."
  cargo build --package logtee $RELEASE_FLAG --target "$TARGET"
done

# macOS universal dylib (arm64 + x86_64) -> SCRIPT_DIR/Logtee.framework
macos_dylib="$BUILD_DIR/lipo-macos/liblogtee.dylib"
mkdir -p "$(dirname "$macos_dylib")"
echo "==> lipo macos dylib"
lipo -create \
  "$BUILD_DIR/aarch64-apple-darwin/$CONFIGURATION/liblogtee.dylib" \
  "$BUILD_DIR/x86_64-apple-darwin/$CONFIGURATION/liblogtee.dylib" \
  -output "$macos_dylib"
bash "$MK_FRAMEWORK" "$macos_dylib" "$SCRIPT_DIR" macos Logtee

# iOS device dylib (arm64 only) -> SCRIPT_DIR/ios/Logtee.framework
bash "$MK_FRAMEWORK" \
  "$BUILD_DIR/aarch64-apple-ios/$CONFIGURATION/liblogtee.dylib" \
  "$SCRIPT_DIR/ios" ios Logtee

echo "==> Done"
