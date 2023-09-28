# Ouisync dart/flutter plugin

A dart/flutter plugin providing high-level dart API for the ouisync native library.

## Building the native library

The native library is built automatically as part of this plugins build
process, but it needs the following prerequisities to be satisfied first:

1. Install [rust](https://www.rust-lang.org/tools/install)
2. For each of the supported platforms, add its corresponding target:

        $ rustup target add $TARGET

Where `$TARGET` is the target triple of the platform (run `rustup target list`
to list all available triples):

    - android arm64:  `aarch64-linux-android`
    - android arm32:  `armv7-linux-androideabi`
    - android x86_64: `x86_64-linux-android`
    - ios arm64:      `aarch64-apple-ios`
    - etc...

## Before using/building this plugin

In order to use this plugin, one must generate the `lib/bindings.g.dart` file:

    $ dart run util/bindgen.dart

Note that the above needs to be done every time the public interface of the
`ouisync_ffi` library changes.

## Building the AAR

Note that one doesn't need to build the `.aar` file manually as it should be
done automatically by the upper level project. See the
[`ouisync-app`](https://github.com/equalitie/ouisync-app/blob/master/pubspec.yaml)
for an example.

If - however - building the standalone `.aar` file is indeed desirable, runnig:

    $ flutter build aar

will create a release, debug and a profile build for `arm32`, `arm64` and
`x86_64` architectures (32bit `x86` is omited by default).

To build only certain architectures or add the missing ones, add the
`--target-platform={android-arm,android-arm64,android-x86,android-x64}` flag to
the above command.

To avoid building the release, debug or profile versions use any combination of
`--no-release`, `--no-debug`, `--no-profile`.
