#!/usr/bin/env bash
# Command line tool which produces a `OuisyncLibFFI` framework for the host's llvm triple.
#
# This tool runs in a sandboxed process that can only write to the `output` folder and cannot access
# the network, so it relies on the `Updater` companion plugin to run a `cargo fetch` before hand.
#
# Called by the tree reconciler plugin which passes its own environment as well as the input, output
# and dependency paths. The tree reconciler checks that the dependency folder exists, but does not
# FIXME: validate that it contains all necessary dependencies as defined in Cargo.toml
#
# Primarily intended for debug builds, the resulting framework is neither cross platform nor cross
# architecture. For distributions purposes, a `fat` framework is needed that includes at the very
# least 2 arm64 targets (iOS and macOS), another x86_64 pair if we want to support older macs and
# iOS simulators and _another_ pair if we want catalyst (x86_64 and arm64)[1].
#
# Weighing in at ~500M, static library seems abnormally large and some 10 times larger than the
# dylib version. It will be stripped down by Xcode but given the above situation, we're currently
# looking at a 3G download, possibly reducible to 1G if we drop catalyst and lipo the platforms but
# FIXME: is there any way to tell cargo to strip the symbols that are not referenced by the ffi?[3]
#
# Additionally, because the tree reconciler can call the builder from a `.prebuildTool()` command,
# this script would have to be included in a `.binaryTarget()`[2] that is statically available at
# package resolution time, and cannot be bundled as a `.executableTarget()` dependency. Currently,
# these binaryTargets must be xcframeworks themselves, so instead of depending on a BuildToolBuilder
# plugin and product from another subpackage which almost certainly results in a dependency loop,
# we're running this file from the package directory after `cargo fetch` completes.
#
# [1] https://forums.developer.apple.com/forums/thread/666335
# [2] https://github.com/swiftlang/swift-package-manager/blob/main/Documentation/Plugins.md#build-tool-target-dependencies
# [3] https://www.amyspark.me/blog/posts/2024/01/10/stripping-rust-libraries.html
TARGET_DIR="$4"
TARGET="debug"

# cargo builds some things that confuse xcode such as fingerprints and depfiles which cannot be
# (easily) disabled; to prevent this, we generate the outputs we care about into $TARGET_DIR/target
# which is a sibling of the $TARGET_DIR/$BUILD_CONFIGURATION folder that cargo generates to
OUTPUT_DIR="$TARGET_DIR/output"
INCLUDE_DIR="$TARGET_DIR/$TARGET/include"

# use a dummy dir to avoid duplicate target errors in xcode
mkdir -p "$TARGET_DIR/dummy"

# prepare include folder
rm -Rf "$INCLUDE_DIR"
mkdir -p "$INCLUDE_DIR"
echo "module OuisyncLibFFI {
    header \"bindings.h\"
    export *
}" > "$INCLUDE_DIR/module.modulemap"

# run cargo in offline mode, using the prefetched dependencies that reside in the same folder as
# this script; xcode calls this with a very limited environment, so the locations of `cargo` and
# `rustc` are passed from the caller
CARGO_HOME="$5" RUSTC="$2" \
"$1" build --offline \
    --package ouisync-ffi \
    --manifest-path "$3" \
    --target-dir "$TARGET_DIR" || exit 1

# generate c bindings (xcode and cargo REALLY don't want to work together)
cd "$(dirname "$3")"
PATH=$(dirname "$1") "$5/bin/cbindgen" --lang C --crate ouisync-ffi > "$INCLUDE_DIR/bindings.h" || exit 2

# delete previous framework (possibly a stub) and replace with new one that contains the archive
# TODO: speed this process up by moving intead of copying; investigate whether a symlink would work
rm -Rf $OUTPUT_DIR/OuisyncLibFFI.xcframework
xcodebuild -create-xcframework \
    -library "$TARGET_DIR/$TARGET/libouisync_ffi.a" \
    -headers "$INCLUDE_DIR" \
    -output "$OUTPUT_DIR/OuisyncLibFFI.xcframework"
