DEBUG=0  # set to 1 if you want to run rust assertions (much slower)
TARGETS=(  # if you're focused on a single target, feel free to disable others
  aarch64-apple-darwin   # mac on apple silicon
  x86_64-apple-darwin    # mac on intel
  aarch64-apple-ios      # all supported devices (ios 11+ are 64 bit only)
#  aarch64-apple-ios-sim  # simulators when running on M chips
#  x86_64-apple-ios       # simulator running on intel chips
)

SKIP=1  # use prebuilt output/ xcframework; do not rebuild via the Xcode plugin

# Running build-xcframework.sh (which sources this file) rebuilds the static
# xcframework AND repackages the dynamic library as OuisyncService.framework for
# each enabled TARGETS entry, into bindings/dart/darwin/{,ios/}. So re-run
# build-xcframework.sh after any Rust API change to refresh both. build-framework.sh
# in bindings/dart/darwin does the dylib->framework wrapping standalone.
