# Ouisync

[![CI](https://github.com/equalitie/ouisync/actions/workflows/ci.yml/badge.svg)](https://github.com/equalitie/ouisync/actions/workflows/ci.yml)
[![dependency status](https://deps.rs/repo/github/equalitie/ouisync/status.svg)](https://deps.rs/repo/github/equalitie/ouisync)

This repository contains the Ouisync library and the Ouisync Command Line Interface (CLI) app. The
GUI app is hosted in a [separate repository](https://github.com/equalitie/ouisync-app),

## What is Ouisync?

[Ouisync](https://ouisync.net) is a secure peer-to-peer file syncing app. It's free, open source and
available for all major desktop and mobile platforms.

## Library and bindings

The Ouisync library is written in rust and is located in the [`lib/`](lib) folder. There are also
bindings available for the following languages:

- [Dart](bindings/dart/)
- [Kotlin](bindings/kotlin)
- [Swift](bindings/swift) (work in progress)

## The Command Line App

The CLI is located in the [`cli/`](cli) folder.

## Testing help acknowlegement

This project is tested with BrowserStack.
