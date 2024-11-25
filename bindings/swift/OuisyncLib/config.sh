# this file is sourced by `build.sh`; keep as many options enabled as you have patience for
SKIP=1
#DEBUG=1  # set to 0 to generate release builds (much faster)
TARGETS=(
  aarch64-apple-darwin   # mac on apple silicon
  x86_64-apple-darwin    # mac on intel
  aarch64-apple-ios      # all real devices (ios 11+ are 64 bit only)
  aarch64-apple-ios-sim  # simulators when running on M chips
  x86_64-apple-ios       # simulator running on intel chips
)
# make sure to re-run "Update rust dependencies" after making changes here
