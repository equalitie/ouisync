# Ouisync

[![CI](https://github.com/equalitie/ouisync/actions/workflows/ci.yml/badge.svg)](https://github.com/equalitie/ouisync/actions/workflows/ci.yml)
[![dependency status](https://deps.rs/repo/github/equalitie/ouisync/status.svg)](https://deps.rs/repo/github/equalitie/ouisync)

## Components

This repository contains two main components: the Ouisync library
([`lib/`](./lib)) containing the core functionality and a command line utility
as a user interface (CLI) for the library ([`cli/`](./cli), currently Linux
only).

There is also a Graphical User Interface (GUI) app for the library hosted in a
[separate repository](https://github.com/equalitie/ouisync-app).

Apart from the above, this repository also contains a C Foreign Function
Interface (FFI) for use by other languages. An example of its use can be found
in the [Flutter based Ouisync
plugin](https://github.com/equalitie/ouisync-plugin) used by the GUI app.

## Building

Note, if you want to build only the CLI application, use the instructions
outlined in the [README.md](./cli/README.md#building) document located in the
[`cli/`](./cli) directory.

Ouisync uses a number of other Rust libraries that are downloaded during the
build process. However, to build and use the Ouisync application (as opposed to
just the library), one will additionally need to install the
[FUSE](https://www.kernel.org/doc/html/latest/filesystems/fuse.html) library
and development files.

    $ sudo apt install pkg-config libfuse-dev

Install Rust using instructions from [rust-lang.org](https://www.rust-lang.org/tools/install).

Build

    $ cargo build --release

The results will then be found in the `./target/release/` directory.

## Debugging tests

Use the `tracing::[level]!` macros for easier debugging.

The format for [`RUST_LOG`](https://docs.rs/env_logger/latest/env_logger/) is either

```
RUST_LOG=[level]
```

or

```
RUST_LOG=[target]=[level],[target]=[level],...
```

Where `level` can be one of `error`, `warn`, `info`, `debug` or `trace`.  The
`target` can be obtained from the list given by `cargo test --package` and by
replacing dashes (`-`) with underscores (`_`) in the package names.

 Ouisync uses
[`tracing_subscriber`](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html)
to include information where individual output lines originated from. To enable
this functionality, set the `level` to either `info`, `debug` or `trace`.

## Testing help acknowlegement

This project is tested with BrowserStack.

