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

# Build a universal (arm64 + x86_64) macOS static lib, not just the host arch —
# Xcode links a universal binary by default, so a single-arch xcframework here
# causes "Undefined symbols for architecture x86_64/arm64" at link time.
TARGETS=(aarch64-apple-darwin x86_64-apple-darwin)

BUILD_DIR="$PROJECT_ROOT/target"
INCLUDE="$BUILD_DIR/swift-include"
XCF="$PACKAGE_DIR/output/OuisyncLibFFI.xcframework"

cd "$PROJECT_ROOT"
for TARGET in "${TARGETS[@]}"; do
    echo "==> Building ouisync-service for $TARGET..."
    "$CARGO" build --package ouisync-service --release --target "$TARGET"
done

ARM64_LIB="$BUILD_DIR/aarch64-apple-darwin/release/libouisync_service.a"
X86_64_LIB="$BUILD_DIR/x86_64-apple-darwin/release/libouisync_service.a"
LIB="$BUILD_DIR/lipo-macos/libouisync_service.a"
mkdir -p "$(dirname "$LIB")"
echo "==> lipo macos: $ARM64_LIB $X86_64_LIB"
lipo -create "$ARM64_LIB" "$X86_64_LIB" -output "$LIB"

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
