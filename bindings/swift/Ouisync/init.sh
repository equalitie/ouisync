#!/usr/bin/env zsh
# This tool is used to prepare the swift build environment; it is necessary due
# to an unfortunate combination of known limitations in the swift package
# manager and git's refusal to permit comitted but gitignored "template files"
# This script must be called before attempting the first `swift build`

# Make sure we have Xcode command line tools installed
xcode-select -p || xcode-select --install

# for build artifacts, swift uses `.build` relative to the swift package and
# `cargo` uses `target` relative to the cargo package (../../../) so we link
# them together to share the build cache and speed builds up whenever possible
cd $(dirname "$0")
BASE="$(realpath .)/.build/plugins/outputs/ouisync/Ouisync/destination"
mkdir -p "$BASE"
CARGO="$(realpath "../../..")/target"
SWIFT="$BASE/CargoBuild"
if [ -d "$CARGO" ]; then
    rm -Rf "$SWIFT"
    mv "$CARGO" "$SWIFT"
else
    rm -f "$CARGO"
    mkdir -p "$SWIFT"
fi
ln -s "$SWIFT" "$CARGO"

# Swift expects some sort of actual framework in the current folder which we
# mock as an empty library with no headers or data that will be replaced before
# it is actually needed via the prebuild tool called during `swift build`
mkdir -p "$SWIFT/OuisyncService.xcframework"
rm -f "OuisyncService.xcframework"
ln -s "$SWIFT/OuisyncService.xcframework" "OuisyncService.xcframework"
cat <<EOF > "OuisyncService.xcframework/Info.plist"
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>AvailableLibraries</key>
    <array>
    </array>
    <key>CFBundlePackageType</key>
    <string>XFWK</string>
    <key>XCFrameworkFormatVersion</key>
    <string>1.0</string>
</dict>
</plist>
EOF

# Even when done incrementally, rust compilation can take considerable time,
# which is amplified by the number of platforms we have to support. There's no
# way around this in the general case, but it's worthwhile to allow developers
# to focus on a single platform in some cases (e.g. when debugging); obviously
# we would like to gitignore this file, but it must exist, so we create it now.
cat <<EOF > "config.sh"
DEBUG=0  # set to 1 if you want to run rust assertions (much slower)
TARGETS=(  # if you're focused on a single target, feel free to disable others
  aarch64-apple-darwin   # mac on apple silicon
#  x86_64-apple-darwin    # mac on intel
#  aarch64-apple-ios      # all supported devices (ios 11+ are 64 bit only)
#  aarch64-apple-ios-sim  # simulators when running on M chips
#  x86_64-apple-ios       # simulator running on intel chips
)
EOF

# Install rust and pull all dependencies neessary for `swift build`
swift package plugin cargo-fetch --allow-network-connections all
