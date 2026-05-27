#!/usr/bin/env bash
# Build OuisyncLibFFI.xcframework for the native macOS host.
# Run this once before `swift test` or opening the package in Xcode.
set -euo pipefail

CARGO_BIN="${CARGO_HOME:-$HOME/.cargo}/bin"
export PATH="$CARGO_BIN:$PATH"
CARGO="$CARGO_BIN/cargo"
CBINDGEN="$CARGO_BIN/cbindgen"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../../" && pwd)"
PACKAGE_DIR="$SCRIPT_DIR"

ARCH="$(uname -m)"
case "$ARCH" in
  arm64)  TARGET="aarch64-apple-darwin" ;;
  x86_64) TARGET="x86_64-apple-darwin" ;;
  *)      echo "Unsupported arch: $ARCH"; exit 1 ;;
esac

BUILD_DIR="$PROJECT_ROOT/target"
LIB="$BUILD_DIR/$TARGET/release/libouisync_service.a"
INCLUDE="$BUILD_DIR/swift-include"
XCF="$PACKAGE_DIR/output/OuisyncLibFFI.xcframework"

echo "==> Building ouisync-service for $TARGET..."
cd "$PROJECT_ROOT"
"$CARGO" build --package ouisync-service --release --target "$TARGET"

echo "==> Generating bindings header..."
mkdir -p "$INCLUDE"
"$CBINDGEN" --lang C \
    --crate ouisync-service \
    --config service/cbindgen.toml \
    > "$INCLUDE/bindings.h"

cat > "$INCLUDE/module.modulemap" <<'EOF'
module OuisyncLibFFI {
    header "bindings.h"
    export *
}
EOF

echo "==> Creating xcframework..."
rm -rf "$XCF"
xcodebuild -create-xcframework \
    -library "$LIB" \
    -headers "$INCLUDE" \
    -output "$XCF"

echo "==> Done: $XCF"
