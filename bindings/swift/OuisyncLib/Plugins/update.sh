#!/usr/bin/env zsh
# Command line tool which pulls all dependencies needed to build the rust core library.
#
# Assumes that `cargo` and `rustup` are installed and available in REAL_PATH and it is run with the
# two plugin output paths (update and build)
PROJECT_HOME=$(realpath "$(dirname "$0")/../../../../")
PACKAGE_HOME=$(realpath "$PROJECT_HOME/bindings/swift/OuisyncLib")
export CARGO_HOME="$1"
export CARGO_HTTP_CHECK_REVOKE="false"  # unclear why this fails, but it does
export RUSTUP_USE_CURL=1  # https://github.com/rust-lang/rustup/issues/1856

# download all possible toolchains: they only take up about 100MiB in total
mkdir -p .rustup
export RUSTUP_HOME="$CARGO_HOME/.rustup"
rustup default stable
rustup target install aarch64-apple-darwin aarch64-apple-ios aarch64-apple-ios-sim \
    x86_64-apple-darwin x86_64-apple-ios

cd "$PROJECT_HOME"
cargo fetch --locked || exit 1  # this is currently only fixable by moving the plugin location
cargo install cbindgen cross || exit 2  # build.sh also needs `cbindgen` and `cross`

# as part of the updater, we also perform the xcode symlink hack: we replace the existing
# $PACKAGE_HOME/output folder (either stub checked out by git or symlink to a previous build) with
# a link to the $BUILD_OUTPUT/output folder which will eventually contain an actual framework
BUILD_OUTPUT="$2"
mkdir -p "$BUILD_OUTPUT"
cd "$BUILD_OUTPUT" > /dev/null
# if this is the first time we build at this location, generate a new stub library to keep xcode
# happy in case the build process fails later down the line
if ! [ -d "output/OuisyncLibFFI.xcframework" ]; then
    "$PACKAGE_HOME/reset-output.sh"
fi

# we can now replace the local stub (or prior link) with a link to the most recent build location
rm -rf "$PACKAGE_HOME/output"
ln -s "$BUILD_OUTPUT/output" "$PACKAGE_HOME/output"

# unfortunately, we can't trigger a build from here because `build.sh` runs in a different sandbox
