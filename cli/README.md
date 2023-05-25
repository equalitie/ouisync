# OuiSync CLI

This directory contains code for OuiSync command line interface (CLI) application. The app can be
build as a standalone binary or as a docker image.

## Building (standalone binary)

Install dependencies

    $ sudo apt install pkg-config libfuse-dev

Install Rust using instructions from [rust-lang.org](https://www.rust-lang.org/tools/install).

Build the app

    $ cargo build --release --bin ouisync

## Building (docker)

See the instructions in `docker/README.md`

## Usage

Run `ouisync --help` to see the available commands. Use `ouisync COMMAND --help` to see detailed
help for the given command.
