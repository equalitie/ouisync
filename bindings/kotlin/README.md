# Ouisync library kotlin bindings

This project provides kotlin bindings for the ouisync library in the form of self-contained AAR
package to be used in android apps.

## Prerequisities

The Ousiync native library is built automatically but it requires a rust toolchain. The easiest way
to get it is using [rustup](https://rustup.rs/).

## Build the AAR

Run `gradle lib:assembleRelease` (or `lib:assembleDebug` for the debug variant), then find the aar
in `build/lib/outputs/aar/lib-release.aar` (or `lib-debug.aar`).

## Run unit tests

Tun `gradle lib:test`

## Example app

There is a simple example app in the `example` folder. To build it run
`gradle example:assembleDebug`, then find the apk in
`build/example/outputs/apk/debug/example-debug.apk`, install and run it on a device or an emulator.

## API documentation

Generate the documentation with [dokka](https://kotlinlang.org/docs/dokka-introduction.html#0):
`gradle lib:dokkaHtml` (or use any of the supported formats) then find it in `build/lib/dokka`.

