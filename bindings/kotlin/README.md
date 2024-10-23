# Ouisync library Kotlin bindings

[![Maven Central Version](https://img.shields.io/maven-central/v/ie.equalit.ouinet/ouisync-omni?label=MavenCentral&logo=apache-maven)](https://search.maven.org/artifact/ie.equalit.ouinet/ouisync-omni)

This project provides kotlin bindings for the ouisync library in the form of self-contained AAR
package to be used in android apps.

## Usage

Include ouisync with Gradle by adding the following to your `build.gradle` file:

```groovy
implementation 'ie.equalit.ouinet:ouisync-omni:$ouisyncVersion'
```

## Documentation

API documentation is available at https://docs.ouisync.net/kotlin/.

## Example app

There is a simple example app in the [example/](example) folder. To build it run
`gradle example:assembleDebug`, then find the apk in
`build/example/outputs/apk/debug/example-debug.apk`, install and run it on a device or an emulator.

## Build from source

### Prerequisities

The Ousiync native library is built automatically but it requires a rust toolchain. The easiest way
to get it is using [rustup](https://rustup.rs/).

### Build the AAR

Run `gradle lib:assembleRelease` (or `lib:assembleDebug` for the debug variant), then find the aar
in `build/lib/outputs/aar/lib-release.aar` (or `lib-debug.aar`).

### Run unit tests

Run `gradle lib:test` (To run a specific test, see [test filtering](https://docs.gradle.org/current/userguide/java_testing.html#test_filtering)).

### Build the API documentation

Generate the documentation with [dokka](https://kotlinlang.org/docs/dokka-introduction.html#0):
`gradle lib:dokkaHtml` (or use any of the supported formats) then find it in `build/lib/dokka`.
