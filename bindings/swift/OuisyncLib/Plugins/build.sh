#!/usr/bin/env zsh
# Command line tool which produces a `OuisyncLibFFI` framework for all configured llvm triples from
# OuisyncLib/config.sh (currently generated in the ouisync-app repository)
#
# This tool runs in a sandboxed process that cannot access the network, so it relies on the updater
# companion plugin to download the required dependencies ahead of time. Called by the builder plugin
# which passes both plugins' output paths as arguments. Hic sunt dracones! These may be of interest:
# [1] https://forums.developer.apple.com/forums/thread/666335
# [2] https://github.com/swiftlang/swift-package-manager/blob/main/Documentation/Plugins.md#build-tool-target-dependencies
# [3] https://www.amyspark.me/blog/posts/2024/01/10/stripping-rust-libraries.html
fatal() { echo "Error $@" && exit $1 }
PROJECT_HOME=$(realpath "$(dirname "$0")/../../../../")
export CARGO_HOME=$(realpath "$1")
export PATH="$CARGO_HOME/bin:$PATH"
export RUSTUP_HOME="$CARGO_HOME/.rustup"
BUILD_OUTPUT=$(realpath "$2")

# cargo builds some things that confuse xcode such as fingerprints and depfiles which cannot be
# (easily) disabled; additionally, xcode does pick up the xcframework and reports it as a duplicate
# target if present in the output folder, so other than the symlink hack from `update.sh`, we have
# to tell xcode that our output is in an empty `dummy` folder
mkdir -p "$BUILD_OUTPUT/dummy"

# read config and prepare to build
source "$PROJECT_HOME/bindings/swift/OuisyncLib/config.sh"
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
    cross build \
        --frozen \
        --package ouisync-ffi \
        --target $TARGET \
        --target-dir "$BUILD_OUTPUT" \
        $FLAGS || fatal 1 "Unable to compile for $TARGET"
done

# generate include files
INCLUDE="$BUILD_OUTPUT/include"
mkdir -p "$INCLUDE"
echo "module OuisyncLibFFI {
    header \"bindings.h\"
    export *
}" > "$INCLUDE/module.modulemap"
cbindgen --lang C --crate ouisync-ffi > "$INCLUDE/bindings.h" || fatal 2 "Unable to generate bindings.h"

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
        lipo -create $MATCHED[@] -output $LIBRARY || fatal 3 "Unable to run lipo for ${MATCHED[@]}"
    fi
    PARAMS+=("-library" "$LIBRARY" "-headers" "$INCLUDE")
done

# TODO: skip xcodebuild and manually create symlinks instead (faster but Info.plist would be tricky)
rm -Rf "$BUILD_OUTPUT/temp.xcframework"
find "$BUILD_OUTPUT/OuisyncLibFFI.xcframework" -mindepth 1 -delete
xcodebuild \
    -create-xcframework ${PARAMS[@]} \
    -output "$BUILD_OUTPUT/temp.xcframework" || fatal 4 "Unable to build xcframework"
for FILE in $(ls "$BUILD_OUTPUT/temp.xcframework"); do
    mv "$BUILD_OUTPUT/temp.xcframework/$FILE" "$BUILD_OUTPUT/OuisyncLibFFI.xcframework/$FILE"
done
