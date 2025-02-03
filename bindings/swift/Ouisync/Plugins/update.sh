#!/usr/bin/env zsh
# Command line tool which pulls all dependencies needed to build the rust core library.
fatal() { echo "Error $@" >&2 && exit $1 }
PROJECT_HOME=$(realpath "$(dirname "$0")/../../../../")
export CARGO_HOME=$(realpath "$1")
export PATH="$CARGO_HOME/bin:$PATH"
export RUSTUP_HOME="$CARGO_HOME/.rustup"

# install rust or update to latest version
export RUSTUP_USE_CURL=1  # https://github.com/rust-lang/rustup/issues/1856
if [ -f "$CARGO_HOME/bin/rustup" ]; then
    rustup update || fatal 1 "Unable to update rust"
else
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path \
    || fatal 1 "Unable to install rust"
fi

# also install all possible toolchains since they only take up about 100MiB in total
export CARGO_HTTP_CHECK_REVOKE="false"  # unclear it fails without this, but it does
rustup target install aarch64-apple-darwin aarch64-apple-ios \
    aarch64-apple-ios-sim x86_64-apple-darwin x86_64-apple-ios || fatal 2 "Unable to install rust via rustup"

# build.sh needs `cbindgen` and `cross` to build as a multiplatform framework
cargo install cbindgen cross || fatal 3 "Unable to install header generator or cross compiler"

# fetch all up to date package dependencies for the next build (which must run offline)
cd "$PROJECT_HOME"
cargo fetch --locked || fatal 4 "Unable to fetch library dependencies"
