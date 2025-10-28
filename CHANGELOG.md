# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased](https://github.com/equalitie/ouisync/compare/v0.9.3...master)

- Improve support for file ownership and permissions on mounted repositories on linux
- Support multiple store directories

## [v0.9.3](https://github.com/equalitie/ouisync/compare/v0.9.2...v0.9.3) - 2025-10-14

- Implement [DocumentsProvider](https://developer.android.com/reference/android/provider/DocumentsProvider) to expose Ouisync repositories to the [Storage Access Framework](https://developer.android.com/guide/topics/providers/document-provider) on Android.
- Fix infinite busy loop when connection fails while establishing topic streams
- Fix crash in `shareFile` in the dart bindings
- Restart the Android foreground service when the app is resumed

## [v0.9.2](https://github.com/equalitie/ouisync/compare/v0.9.1...v0.9.2) - 2025-08-05

- Use the same config and store directories as the [GUI app](https://github.com/equalitie/ouisync-app).
- Improve some error messages in the CLI app.
- Introduce the [logtee](https://github.com/equalitie/ouisync/tree/master/logtee) library for
  capturing log messages from multiple sources
- Fix crash on older Linux and Android versions due to missing libc function
- Fix crash due to race condition when concurrently opening the same repo multiple times
- Fix kotlin API docs not being generated completely
- Fix notification events being sometimes spammed indefinitely after creating new directory
- Gracefully handle [foreground service timeout](https://developer.android.com/develop/background-work/services/fgs/timeout)
- Unmount repositories prior to deletion

## [v0.9.1](https://github.com/equalitie/ouisync/compare/v0.9.0...v0.9.1) - 2025-06-26

- Fix panics when accessing the metrics endpoint.
- Fix repositories not being closed cleanly on shutdown of the CLI app

## [v0.9.0](https://github.com/equalitie/ouisync/compare/v0.8.11...v0.9.0) - 2025-06-05

### Breaking changes

- This release contains breaking changes in the syncing protocol and so apps on this version won't
  be able to sync with those on the previous version.
- Changed syntax of some of the CLI commands.
- Using the CLI app now requires running Ouisync server. This limitation may be lifted
  in a future version.

### Other changes

- Ongoing work towards improving iOS and macOS support.
- Implement `ouisync-service` to exposes the Ouisync API over a simple IPC protocol used as a bridge
  between the Ouisync library and various language bindings. Also implement automatic generation of
  the public API for such bindings.
- Implement Android [Service](https://developer.android.com/develop/background-work/services) to
  support syncing in the background on newer versions of Android.
- Refactor and optimize the low-level networking module.
- Implement an algorithm to better distribute the network load between peers and improve sync
  performance
- Fix [quinn](https://github.com/quinn-rs/quinn) bugs causing syncing to sometimes stop on Android
  and Linux.
- Fix mount failure if repo not unmounted cleanly during the previous run of the app on Linux.
- Fix file sizes being reported incorrectly by some Linux utilities (https://github.com/equalitie/ouisync/issues/173).
- Update dependencies to their latest versions

## [v0.8.11](https://github.com/equalitie/ouisync/compare/v0.8.10...v0.8.11) - 2025-01-27

- Add support for EC private keys for TLS.
- Update [NDK](https://developer.android.com/ndk) to 27.2.12479018.

# Appendix A: Git tags reference

This section lists older tags in the repository as a reference to the changes
released before adhering to consistent release note format. A link to the
comparison between the previous version and the listed version is provided to
give some idea of the changes included in the version. 

## [v0.8.10](https://github.com/equalitie/ouisync/releases/tag/v0.8.10) - 2024-11-06

- See changes [v0.8.9...v0.8.10](https://github.com/equalitie/ouisync/compare/v0.8.9...v0.8.10)

## [v0.8.9](https://github.com/equalitie/ouisync/releases/tag/v0.8.9) - 2024-11-04

- See changes [v0.8.8...v0.8.9](https://github.com/equalitie/ouisync/compare/v0.8.8...v0.8.9)

## [v0.8.8](https://github.com/equalitie/ouisync/releases/tag/v0.8.8) - 2024-07-05

- See changes [v0.8.7...v0.8.8](https://github.com/equalitie/ouisync/compare/v0.8.7...v0.8.8)

## [v0.8.7](https://github.com/equalitie/ouisync/releases/tag/v0.8.7) - 2024-06-19

- See changes [v0.8.6...v0.8.7](https://github.com/equalitie/ouisync/compare/v0.8.6...v0.8.7)

## [v0.8.6](https://github.com/equalitie/ouisync/releases/tag/v0.8.6) - 2024-06-18

- See changes [v0.8.5...v0.8.6](https://github.com/equalitie/ouisync/compare/v0.8.5...v0.8.6)

## [v0.8.5](https://github.com/equalitie/ouisync/releases/tag/v0.8.5) - 2024-06-13

- See changes [v0.8.4...v0.8.5](https://github.com/equalitie/ouisync/compare/v0.8.4...v0.8.5)

## [v0.8.4](https://github.com/equalitie/ouisync/releases/tag/v0.8.4) - 2024-06-13

- See changes [v0.8.3...v0.8.4](https://github.com/equalitie/ouisync/compare/v0.8.3...v0.8.4)

## [v0.8.3](https://github.com/equalitie/ouisync/releases/tag/v0.8.3) - 2024-05-28

- See changes [v0.8.2...v0.8.3](https://github.com/equalitie/ouisync/compare/v0.8.2...v0.8.3)

## [v0.8.2](https://github.com/equalitie/ouisync/releases/tag/v0.8.2) - 2024-04-03

- See changes [v0.8.1...v0.8.2](https://github.com/equalitie/ouisync/compare/v0.8.1...v0.8.2)

## [v0.8.1](https://github.com/equalitie/ouisync/releases/tag/v0.8.1) - 2024-03-15

- See changes [v0.8.0...v0.8.1](https://github.com/equalitie/ouisync/compare/v0.8.0...v0.8.1)

## [v0.8.0](https://github.com/equalitie/ouisync/releases/tag/v0.8.0) - 2024-03-12

- See changes [v0.7.4...v0.8.0](https://github.com/equalitie/ouisync/compare/v0.7.4...v0.8.0)

## [v0.7.4](https://github.com/equalitie/ouisync/releases/tag/v0.7.4) - 2024-01-25

- See changes [v0.7.3...v0.7.4](https://github.com/equalitie/ouisync/compare/v0.7.3...v0.7.4)

## [v0.7.3](https://github.com/equalitie/ouisync/releases/tag/v0.7.3) - 2024-01-18

- See changes [v0.7.2...v0.7.3](https://github.com/equalitie/ouisync/compare/v0.7.2...v0.7.3)

## [v0.7.2](https://github.com/equalitie/ouisync/releases/tag/v0.7.2) - 2023-12-05

- See changes [v0.7.1...v0.7.2](https://github.com/equalitie/ouisync/compare/v0.7.1...v0.7.2)

## [v0.7.1](https://github.com/equalitie/ouisync/releases/tag/v0.7.1) - 2023-12-05

- See changes [v0.7.0...v0.7.1](https://github.com/equalitie/ouisync/compare/v0.7.0...v0.7.1)

## [v0.7.0](https://github.com/equalitie/ouisync/releases/tag/v0.7.0) - 2023-12-05

- See changes [v0.6.1...v0.7.0](https://github.com/equalitie/ouisync/compare/v0.6.1...v0.7.0)

## [v0.6.1](https://github.com/equalitie/ouisync/releases/tag/v0.6.1) - 2023-11-02

- See changes [v0.6.0...v0.6.1](https://github.com/equalitie/ouisync/compare/v0.6.0...v0.6.1)

## [v0.6.0](https://github.com/equalitie/ouisync/releases/tag/v0.6.0) - 2023-11-02

- First tagged release
- See changes [v0.5.2...v0.6.1](https://github.com/equalitie/ouisync/compare/fc501bd...v0.6.1)

## [v0.5.2](https://github.com/equalitie/ouisync/commit/fc501bd) - 2023-06-27

- See changes [v0.5.1...v0.5.2](https://github.com/equalitie/ouisync/compare/c040ba2...fc501bd)

## [v0.5.1](https://github.com/equalitie/ouisync/commit/c040ba2) - 2022-12-21

- See changes [v0.5.0...v0.5.1](https://github.com/equalitie/ouisync/compare/45d762e...c040ba2)

## [v0.5.0](https://github.com/equalitie/ouisync/commit/45d762e) - 2022-11-23

- See changes [v0.5.0...v0.5.1](https://github.com/equalitie/ouisync/compare/c5b90b5...45d762e)

## [v0.4.1](https://github.com/equalitie/ouisync/commit/c5b90b5) - 2022-11-22

- See changes [v0.4.0...v0.4.1](https://github.com/equalitie/ouisync/compare/6ecf303...c5b90b5)

## [v0.4.0](https://github.com/equalitie/ouisync/commit/6ecf303) - 2022-10-18

- See changes [v0.3.1...v0.4.0](https://github.com/equalitie/ouisync/compare/0210198...6ecf303)

## [v0.3.1](https://github.com/equalitie/ouisync/commit/0210198) - 2022-10-05

- See changes [v0.2.0...v0.3.1](https://github.com/equalitie/ouisync/compare/4d5a538...0210198)

## [v0.2.0](https://github.com/equalitie/ouisync/commit/4d5a538) - 2022-10-05

- See changes [653675e...v0.2.0](https://github.com/equalitie/ouisync/compare/653675e...4d5a538)

## [653675e](https://github.com/equalitie/ouisync/commit/653675e) - 2020-09-16

- Initial commit
