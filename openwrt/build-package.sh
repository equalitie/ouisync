#!/bin/sh

set -eu

die() {
    printf 'openwrt: %s\n' "$*" >&2
    exit 1
}

require_command() {
    command -v "$1" >/dev/null 2>&1 || die "required command not found: $1"
}

[ "$#" -ge 4 ] || die 'usage: build-package.sh RUST_TARGET OPENWRT_ARCH VERSION ipk|apk ...'
cd "$(dirname "$0")/.."

target=$1
arch=$2
version=$3
shift 3

cross=${CROSS:-cross}
apk=${APK:-apk}

pkg=ouinode
config=/etc/ouinode/store_dirs.conf
description='Ouisync node for OpenWrt'
name="${pkg}_${version}_${arch}"

case "$arch" in
    ''|*[!a-zA-Z0-9_-]*) die "invalid OpenWrt architecture: $arch" ;;
esac

printf '%s\n' "$version" | grep -Eq '^[0-9]+(\.[0-9]+)*(_(alpha|beta|pre|rc|p|git)[0-9]*)?-r[0-9]+$' ||
    die "invalid package version: $version (use e.g. 0.10.2-r1)"

for tool in rustup rustc cargo "$cross" readelf; do
    require_command "$tool"
done

rustup which rustc >/dev/null || die 'no active Rust toolchain; install it with rustup'
rustup which cargo >/dev/null || die 'Cargo is missing from the active Rust toolchain'

toolchain=
build_std=false

case "$target" in
    mips-unknown-linux-musl|mipsel-unknown-linux-musl)
        toolchain=+nightly
        build_std=std
        if ! rustup which --toolchain nightly rustc >/dev/null ||
           ! rustup which --toolchain nightly cargo >/dev/null; then
            die 'MIPS requires an installed nightly toolchain with Rust and Cargo'
        fi

        sysroot=$(rustc +nightly --print sysroot)
        [ -f "$sysroot/lib/rustlib/src/rust/library/Cargo.toml" ] ||
            die 'MIPS requires rust-src: rustup component add rust-src --toolchain nightly'
        ;;
    aarch64-unknown-linux-musl|armv7-unknown-linux-musleabihf)
        installed=$(rustup target list --installed)
        printf '%s\n' "$installed" | grep -Fxq "$target" ||
            die "Rust target is missing: rustup target add $target"
        ;;
    *) die "unsupported Rust target: $target" ;;
esac

make_ipk=false
make_apk=false

for format do
    case "$format" in
        ipk) make_ipk=true ;;
        apk) make_apk=true ;;
        *) die "unsupported package format: $format" ;;
    esac
done

if "$make_apk"; then
    require_command "$apk"
    apk_version=$("$apk" --version) || die 'apk is not usable'
    case "$apk_version" in
        'apk-tools 3.'*) ;;
        *) die 'apk-tools v3 with apk mkpkg is required' ;;
    esac

    if [ "$(id -u)" != 0 ]; then
        require_command fakeroot
    fi
fi

umask 022
mkdir -p openwrt/dist
dist="$PWD/openwrt/dist"
work=$(mktemp -d "$dist/.build.XXXXXXXX")
trap 'rm -rf "$work"' EXIT
trap 'exit 1' HUP INT TERM

unset CARGO_ENCODED_RUSTFLAGS
CROSS_NO_WARNINGS=1 CROSS_BUILD_STD="$build_std" \
    RUSTFLAGS='-C target-feature=+crt-static' \
    "$cross" ${toolchain:+"$toolchain"} build --locked --release --target "$target" \
        --target-dir "$work/target" --package ouisync-cli --bin ouisync --no-default-features \
        --config profile.release.strip=true

payload="$work/payload"
binary="$work/target/$target/release/ouisync"
elf=$(LC_ALL=C readelf -h -l -d "$binary") || die "cannot read built binary: $binary"

case "$target" in
    aarch64-*) machine=AArch64; endian=little ;;
    armv7-*) machine=ARM; endian=little ;;
    mips-*) machine=MIPS; endian=big ;;
    mipsel-*) machine=MIPS; endian=little ;;
esac

if ! printf '%s\n' "$elf" | grep -Eq "Machine: +$machine( |$)" ||
   ! printf '%s\n' "$elf" | grep -Eq "Data: .* $endian endian"; then
    die "unexpected ELF architecture for $target"
fi

if [ "$machine" = ARM ] && ! printf '%s\n' "$elf" | grep -Fq 'hard-float ABI'; then
    die "expected hard-float ABI for $target"
fi

if printf '%s\n' "$elf" | grep -Eq 'INTERP|\(NEEDED\)'; then
    die "expected a statically linked binary for $target"
fi

install -Dm755 "$binary" "$payload/usr/bin/ouisync"
install -Dm755 openwrt/files/ouinode.init "$payload/etc/init.d/ouinode"
install -d -m700 "$payload/etc/ouinode"
install -m600 openwrt/files/store_dirs.conf "$payload$config"

pack_ipk() {
    mkdir "$work/control"
    tar --format=gnu --numeric-owner --owner=0 --group=0 -cf "$work/data.tar" -C "$payload" .
    bytes=$(wc -c < "$work/data.tar")
    gzip -n "$work/data.tar"

    printf '%s\n' "Package: $pkg" "Version: $version" "Architecture: $arch" \
        'Section: net' 'License: MPL-2.0' 'Depends: procd' "Installed-Size: $bytes" \
        "Description: $description" > "$work/control/control"
    printf '%s\n' "$config" > "$work/control/conffiles"
    install -m755 openwrt/files/remove.sh "$work/control/prerm"
    install -m755 openwrt/files/restart.sh "$work/control/postinst"

    printf '2.0\n' > "$work/debian-binary"
    tar --format=gnu --numeric-owner --owner=0 --group=0 -czf "$work/control.tar.gz" -C "$work/control" .
    tar --format=gnu --numeric-owner --owner=0 --group=0 -czf "$work/$name.ipk" -C "$work" \
        ./debian-binary ./data.tar.gz ./control.tar.gz
}

pack_apk() {
    metadata="$payload/lib/apk/packages"
    mkdir -p "$metadata"

    printf '%s\n' "$config" > "$metadata/$pkg.conffiles"
    checksum=$(sha256sum "$payload$config")
    printf '%s %s\n' "$config" "${checksum%% *}" > "$metadata/$pkg.conffiles_static"

    find "$payload" -type f,l -printf '/%P\n' > "$work/files.list"
    LC_ALL=C sort "$work/files.list" > "$metadata/$pkg.list"

    set -- "$apk" mkpkg --no-xattrs --info "name:$pkg" --info "version:$version" --info "arch:$arch" \
        --info "description:$description" --info 'license:MPL-2.0' --info 'depends:procd' \
        --script "pre-deinstall:$PWD/openwrt/files/remove.sh" \
        --script "post-upgrade:$PWD/openwrt/files/restart.sh" \
        --files "$payload" --output "$work/$name.apk"

    if [ "$(id -u)" = 0 ]; then
        chown -R 0:0 "$payload"
        "$@"
    else
        fakeroot sh -ec 'chown -R 0:0 "$1"; shift; exec "$@"' sh "$payload" "$@"
    fi
}

publish() {
    format=$1
    mv -f "$work/$name.$format" "$dist/$name.$format"
    printf 'Built %s\n' "$dist/$name.$format"
}

if "$make_ipk"; then pack_ipk; fi
if "$make_apk"; then pack_apk; fi

if "$make_ipk"; then publish ipk; fi
if "$make_apk"; then publish apk; fi
