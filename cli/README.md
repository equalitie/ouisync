# Ouisync CLI

This directory contains code for the Ouisync command line interface (CLI) application which can be
built as a standalone binary or as a Docker image.

## Building (standalone binary)

1. Install dependencies:
   
`sudo apt install pkg-config libfuse-dev`

3. Install Rust using instructions from [rust-lang.org](https://www.rust-lang.org/tools/install).

4. Build the app:
   
`cargo build --release --bin ouisync`

## Building (docker)

See the instructions in [`docker/README.md`](https://github.com/equalitie/ouisync/blob/master/docker/README.md).

## Usage

Run `ouisync --help` to see the available commands. Use `ouisync COMMAND --help` to see detailed
help for the given command.
