#!/usr/bin/env zsh
# Command line tool which produces a OuisyncService xcframework for all llvm
# triples from config.sh (defaults to macos x86_64, macos arm64 and ios arm64)
#
# This tool runs in a sandboxed process that cannot access the network, so it
# relies on the updater companion plugin to download the required dependencies
# ahead of time. Hic sunt dracones! These may be of interest:
# [1] https://forums.developer.apple.com/forums/thread/666335
# [2] https://github.com/swiftlang/swift-package-manager/blob/main/Documentation/Plugins.md#build-tool-target-dependencies
# [3] https://www.amyspark.me/blog/posts/2024/01/10/stripping-rust-libraries.html
fatal() { echo "Error $@" && exit $1 }
PROJECT_HOME=$(realpath "$(dirname "$0")/../../../../")
export MACOSX_DEPLOYMENT_TARGET=13.0
BUILD_OUTPUT=$(realpath "$1")

# find the rust toolchain installed by `update.sh` by searching for swift's
# package plugin root directory (called "plugins") in the build output path
cd "$BUILD_OUTPUT";
while : ; do
    PWD=$(basename "$(pwd)")
    test "$PWD" != / || fatal 1 "Unable to find swift package plugin root"
    test "$PWD" != "plugins" || break
    cd ..
done
CROSS=$(find . -path "**/bin/cross" -print -quit)
test -f $CROSS || fatal 2 "Please run `Update rust dependencies` on the Ouisync package"
export CARGO_HOME=$(realpath "$(dirname "$(dirname "$CROSS")")")
export PATH="$CARGO_HOME/bin:$PATH"
export RUSTUP_HOME="$CARGO_HOME/.rustup"

# cargo builds some things that confuse xcode such as fingerprints and depfiles
# which cannot be (easily) disabled; additionally, xcode does pick up the
# xcframework and reports it as a duplicate target if present in the output
# folder, so we tell xcode that all our output is in this empty `dummy` folder
mkdir -p "$BUILD_OUTPUT/dummy"

# read config and prepare to build
source "$PROJECT_HOME/bindings/swift/Ouisync/config.sh" || fatal 1 "Unable to find config file"
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
cd "$PROJECT_HOME"
for TARGET in ${(k)TARGETS}; do
    cross build \
        --frozen \
        --package ouisync-service \
        --target $TARGET \
        --target-dir "$BUILD_OUTPUT" \
        $FLAGS || fatal 3 "Unable to compile for $TARGET"
done

# generate include files
INCLUDE="$BUILD_OUTPUT/include"
mkdir -p "$INCLUDE"
echo "module OuisyncService {
    header \"bindings.h\"
    export *
}" > "$INCLUDE/module.modulemap"
cbindgen --lang C --crate ouisync-service > "$INCLUDE/bindings.h" || fatal 4 "Unable to generate bindings.h"
# hack for autoimporting enums https://stackoverflow.com/questions/60559599/swift-c-api-enum-in-swift
perl -i -p0e 's/enum\s+(\w+)([^}]+});\ntypedef (\w+) \1/typedef enum __attribute__\(\(enum_extensibility\(open\)\)\) : \3\2 \1/sg' "$INCLUDE/bindings.h"

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
            MATCHED+="$BUILD_OUTPUT/$TARGET/$CONFIGURATION/libouisync_service.a"
        fi
    done
    if [ $#MATCHED -eq 0 ]; then  # platform not enabled
        continue
    elif [ $#MATCHED -eq 1 ]; then  # single architecture: skip lipo and link directly
        LIBRARY=$MATCHED
    else  # at least two architectures; run lipo on all matches and link the output instead
        LIBRARY="$BUILD_OUTPUT/$PLATFORM/libouisync_service.a"
        mkdir -p "$(dirname "$LIBRARY")"
        lipo -create $MATCHED[@] -output $LIBRARY || fatal 5 "Unable to run lipo for ${MATCHED[@]}"
    fi
    PARAMS+=("-library" "$LIBRARY" "-headers" "$INCLUDE")
done

# TODO: skip xcodebuild and manually create symlinks instead (faster but Info.plist would be tricky)
rm -Rf "$BUILD_OUTPUT/temp.xcframework"
find "$BUILD_OUTPUT/OuisyncService.xcframework" -mindepth 1 -delete
xcodebuild \
    -create-xcframework ${PARAMS[@]} \
    -output "$BUILD_OUTPUT/temp.xcframework" || fatal 6 "Unable to build xcframework"
for FILE in $(ls "$BUILD_OUTPUT/temp.xcframework"); do
    mv "$BUILD_OUTPUT/temp.xcframework/$FILE" "$BUILD_OUTPUT/OuisyncService.xcframework/$FILE"
done
