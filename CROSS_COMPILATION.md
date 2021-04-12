The easiest way to cross-compile this project for various platforms is to use the [cargo-cross](https://github.com/rust-embedded/cross) tool:

    cross build [FLAGS] --target <TARGET>

Supported targets: https://github.com/rust-embedded/cross#supported-targets.

## Common examples:

Android:

(note: use `--lib` to build only the libraries and not the binary which would be useless for android)

- `cross build --release --lib --target aarch64-linux-android`
- `cross build --release --lib --target arm-linux-androideabi`
- `cross build --release --lib --target armv7-linux-androideabi`
- `cross build --release --lib --target i686-linux-android`
- `cross build --release --lib --target x86_64-linux-android`

Windows:

- `cross build --release --bins --target x86_64-pc-windows-gnu`

Raspberry PI with GNU libc:

- `cross build --release --bins --target armv7-unknown-linux-gnueabihf`

Raspberry PI with musl libc:

- `cross build --release --bins --target armv7-unknown-linux-muslhf`

iOS (must be run on a mac):

- `cross build --release --lib --target aarch64-apple-ios`
- `cross build --release --lib --target armv7-apple-ios`
- `cross build --release --lib --target armv7s-apple-ios`
- `cross build --release --lib --target i386-apple-ios`
- `cross build --release --lib --target x86_64-apple-ios`

etc...