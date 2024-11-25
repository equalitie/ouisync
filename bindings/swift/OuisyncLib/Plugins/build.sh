#!/usr/bin/env zsh
# Command line tool which produces a `OuisyncLibFFI` framework for all configured llvm triples from
# OuisyncLib/config.sh
#
# This tool runs in a sandboxed process that can only write to a `output` folder and cannot access
# the network, so it relies on the `Updater` companion plugin to download the required dependencies
# before hand. Unfortunately, this does not work 100% of the time since both rust and especially
# cargo like to touch the lockfiles or the network for various reasons even when told not to.
#
# Called by the builder plugin which passes its own environment as well as the input, dependency
# and output paths. The builder checks that the dependency folder exists, but does not otherwise
# FIXME: validate that it contains all necessary dependencies as defined in Cargo.toml
#
# Hic sunt dracones! These might be of interest to anyone thinking they can do better than this mess:
#
# [1] https://forums.developer.apple.com/forums/thread/666335
# [2] https://github.com/swiftlang/swift-package-manager/blob/main/Documentation/Plugins.md#build-tool-target-dependencies
# [3] https://www.amyspark.me/blog/posts/2024/01/10/stripping-rust-libraries.html
PROJECT_HOME=$(realpath "$(dirname "$0")/../../../../")
PACKAGE_HOME=$(realpath "$PROJECT_HOME/bindings/swift/OuisyncLib")
export CARGO_HOME="$1"
export RUSTUP_HOME="$CARGO_HOME/.rustup"
BUILD_OUTPUT="$2"

# cargo builds some things that confuse xcode such as fingerprints and depfiles which cannot be
# (easily) disabled; additionally, xcode does pick up the xcframework and reports it as a duplicate
# target if present in the output folder, so other than the symlink hack from `update.sh`, we have
# to tell xcode that our output is in an empty `dummy` folder
mkdir -p "$BUILD_OUTPUT/dummy"

# read config and prepare to build
source "$PACKAGE_HOME/config.sh"
if [ $SKIP ] && [ $SKIP -gt 0 ]; then
    exit 0
fi
if [ $DEBUG ] && [ $DEBUG -gt 0 ]; then
    CONFIGURATION="debug"
    FLAGS=""
else
    CONFIGURATION="release"
    FLAGS="--release"
fi

# convert targets to dictionary
LIST=($TARGETS[@])
declare -A TARGETS
for TARGET in $LIST[@]; do TARGETS[$TARGET]="" done

# build configured targets
cd $PROJECT_HOME
for TARGET in ${(k)TARGETS}; do
    "$CARGO_HOME/bin/cross" build \
        --frozen \
        --package ouisync-ffi \
        --target $TARGET \
        --target-dir "$BUILD_OUTPUT" \
        $FLAGS || exit 1
done

# generate include files
INCLUDE="$BUILD_OUTPUT/include"
mkdir -p "$INCLUDE"
echo "module OuisyncLibFFI {
    header \"bindings.h\"
    export *
}" > "$INCLUDE/module.modulemap"
"$CARGO_HOME/bin/cbindgen" --lang C --crate ouisync-ffi > "$INCLUDE/bindings.h" || exit 2

# delete previous framework (possibly a stub) and replace with new one that contains the archive
# TODO: some symlinks would be lovely here instead, cargo already create two copies
rm -Rf $BUILD_OUTPUT/output/OuisyncLibFFI.xcframework

# xcodebuild refuses multiple architectures per platform, instead expecting fat libraries when the
# destination operating system supports multiple architectures; apple also explicitly rejects any
# submissions that link to mixed-platform libraries so `lipo` usage is reduced to an if and only if
# scenario; since our input is a list of llvm triples which do not follow rigid naming conventions,
# we first have to statically define the platform->arch tree and then run some annoying diffs on it
PARAMS=()
declare -A TREE
TREE=(
  macos "aarch64-apple-darwin x86_64-apple-darwin"
  ios "aarch64-apple-ios"
  simulator "aarch64-apple-ios-sim x86_64-apple-ios"
)
for PLATFORM OUTPUTS in ${(kv)TREE}; do
    MATCHED=()  # list of libraries compiled for this platform
    for TARGET in ${=OUTPUTS}; do
        if [[ -v TARGETS[$TARGET] ]]; then
            MATCHED+="$BUILD_OUTPUT/$TARGET/$CONFIGURATION/libouisync_ffi.a"
        fi
    done
    if [ $#MATCHED -eq 0 ]; then  # platform not enabled
        continue
    elif [ $#MATCHED -eq 1 ]; then  # single architecture: skip lipo and link directly
        LIBRARY=$MATCHED
    else  # at least two architectures; run lipo on all matches and link the output instead
        LIBRARY="$BUILD_OUTPUT/$PLATFORM/libouisync_ffi.a"
        mkdir -p "$(dirname "$LIBRARY")"
        lipo -create $MATCHED[@] -output $LIBRARY || exit 3
    fi
    PARAMS+=("-library" "$LIBRARY" "-headers" "$INCLUDE")
done
echo ${PARAMS[@]}
xcodebuild \
    -create-xcframework ${PARAMS[@]} \
    -output "$BUILD_OUTPUT/output/OuisyncLibFFI.xcframework" || exit 4
