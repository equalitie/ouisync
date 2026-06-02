#!/usr/bin/env bash
# Build OuisyncLibFFI.xcframework for all platforms listed in config.sh.
# Run this once before `swift build`, `swift test`, or opening the package in Xcode.
# Re-run any time the Rust service API changes.
#
# Targets are read from config.sh (TARGETS array). SKIP is intentionally ignored —
# this script always builds. Set DEBUG=1 in config.sh or in the environment for a
# debug build (much faster, but not suitable for production).
#
# Prerequisites:
#   cargo, cbindgen, rustup
#   Rust targets: rustup target add aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios
#   Xcode (not just CLT): xcode-select -p must print …/Xcode.app/Contents/Developer
set -euo pipefail

CARGO_BIN="${CARGO_HOME:-$HOME/.cargo}/bin"
export PATH="$CARGO_BIN:$PATH"
CARGO="$CARGO_BIN/cargo"
CBINDGEN="$CARGO_BIN/cbindgen"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../../" && pwd)"
PACKAGE_DIR="$SCRIPT_DIR"

# Read TARGETS (and optional DEBUG) from config.sh; SKIP is deliberately not used here.
source "$SCRIPT_DIR/config.sh"

if [[ "${DEBUG:-}" == "1" ]]; then
    CONFIGURATION="debug"
    RELEASE_FLAG=""
else
    CONFIGURATION="release"
    RELEASE_FLAG="--release"
fi

BUILD_DIR="$PROJECT_ROOT/target"
INCLUDE="$BUILD_DIR/swift-include"
XCF="$PACKAGE_DIR/output/OuisyncLibFFI.xcframework"

# ── 1. Build each enabled target ──────────────────────────────────────────────
cd "$PROJECT_ROOT"
for TARGET in "${TARGETS[@]}"; do
    echo "==> Building ouisync-service for $TARGET..."
    "$CARGO" build --package ouisync-service $RELEASE_FLAG --target "$TARGET"
done

# ── 2. Generate C header and module map ───────────────────────────────────────
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

# ── 3. Assemble xcframework ────────────────────────────────────────────────────
echo "==> Creating xcframework..."
rm -rf "$XCF"
XCF_PARAMS=()

# Returns 0 if $1 is listed in the TARGETS array, 1 otherwise.
target_enabled() {
    local needle="$1"
    for t in "${TARGETS[@]}"; do
        [[ "$t" == "$needle" ]] && return 0
    done
    return 1
}

# Adds a -library/-headers pair to XCF_PARAMS for the given list of .a files.
# Runs lipo when more than one library is supplied.
add_slice() {
    local name="$1"; shift   # logical name used as lipo output dir
    local libs=("$@")
    [[ ${#libs[@]} -eq 0 ]] && return 0

    local library
    if [[ ${#libs[@]} -eq 1 ]]; then
        library="${libs[0]}"
    else
        library="$BUILD_DIR/lipo-$name/libouisync_service.a"
        mkdir -p "$(dirname "$library")"
        echo "==> lipo $name: ${libs[*]}"
        lipo -create "${libs[@]}" -output "$library"
    fi
    XCF_PARAMS+=("-library" "$library" "-headers" "$INCLUDE")
}

# macOS: universal binary from arm64 + x86_64
macos_libs=()
for TARGET in aarch64-apple-darwin x86_64-apple-darwin; do
    if target_enabled "$TARGET"; then
        macos_libs+=("$BUILD_DIR/$TARGET/$CONFIGURATION/libouisync_service.a")
    fi
done
add_slice macos "${macos_libs[@]}"

# iOS device: arm64 only (no lipo needed)
ios_libs=()
if target_enabled aarch64-apple-ios; then
    ios_libs+=("$BUILD_DIR/aarch64-apple-ios/$CONFIGURATION/libouisync_service.a")
fi
add_slice ios "${ios_libs[@]}"

# iOS simulator: universal binary from arm64-sim + x86_64
sim_libs=()
for TARGET in aarch64-apple-ios-sim x86_64-apple-ios; do
    if target_enabled "$TARGET"; then
        sim_libs+=("$BUILD_DIR/$TARGET/$CONFIGURATION/libouisync_service.a")
    fi
done
add_slice ios-simulator "${sim_libs[@]}"

xcodebuild -create-xcframework "${XCF_PARAMS[@]}" -output "$XCF"
echo "==> Done: $XCF"
