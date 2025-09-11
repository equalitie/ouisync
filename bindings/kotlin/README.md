# Ouisync Kotlin bindings

This project provides kotlin bindings for the Ouisync library. It consist of these packages:

- **ouisync-service** provides the Ouisync *service* which maintains the repositories and runs the
    sync protocol. It can be interacted with using *sessions*.
- **ouisync-session** is the entry point to Ouisync. It's used to manage the repositories, access
    their content and configure the sync protocol, among other things. Multiple *sessions* can
    connect to the same *service*, even across process boundaries.
- **ouisync-android** provides high-level components for developing Android apps:
    [foreground service](https://developer.android.com/develop/background-work/services/fgs) and
    [documents provider](https://developer.android.com/guide/topics/providers/document-provider#overview).

## Installation

The packages are published on [Maven Central](https://central.sonatype.com/). Add them as
dependencies to your project:

```groovy
dependencies {
    implementation "ie.equalit.ouinet:ie.equalit.ouinet:ouisync-session:$ouisync_version"
    implementation "ie.equalit.ouinet:ie.equalit.ouinet:ouisync-service:$ouisync_version"
    implementation "ie.equalit.ouinet:ie.equalit.ouinet:ouisync-android:$ouisync_version"
}
```

Replace `$ouisync_version` with the version of Ouisync you want to use.

## API documentation

Documentation is available at [docs.ouisync.net](https://docs.ouisync.net/kotlin).

## Examples

A simple example app is in the
[bindings/kotlin/example](https://github.com/equalitie/ouisync/tree/master/bindings/kotlin/example)
folder. To build it run `gradle example:assembleDebug`. Find the apk in
`build/example/outputs/apk/debug/example-debug.apk`, install and run it on a device or an emulator.

## Build from source

### Prerequisities

The Ousiync native library is built automatically but it requires a rust toolchain. The easiest way
to get it is using [rustup](https://rustup.rs/).

### Build packages

To build all packages in all variants (release, debug), run `gradle assembleRelease` from inside the
`bindings/kotlin` folder. To see other available tasks, run `gradle tasks`.
