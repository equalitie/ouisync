# ouisync-service (Python binding)

The service half of the Python binding: an in-process embedding of the Ouisync service.
`bindings/python/session` is the client half.

Once started, a `Service` doesn't do anything on its own -- interact with it
by connecting a [`Session`][ouisync.session.Session] (from `ouisync-session`)
to the same config directory, same as with an externally-run `ouisync`
process.

## Usage

```python
from ouisync.service import Service
from ouisync.session import connect

service = await Service.start("/path/to/config")
session = await connect("/path/to/config")
...
```

## Building and loading the native library

The native library is built automatically via a build hook (`hatch_build.py`) and bundled into the
wheel (next to `ouisync/service`), so `pip install ouisync-service` doesn't require a separate Rust
build step. Each wheel is platform-specific, built for exactly one Rust target triple:

- `x86_64-unknown-linux-gnu`
- `aarch64-unknown-linux-gnu`
- `x86_64-pc-windows-msvc` / `x86_64-pc-windows-gnu`
- `aarch64-pc-windows-msvc` / `aarch64-pc-windows-gnu`
- `x86_64-apple-darwin`
- `aarch64-apple-darwin`

By default the hook builds for the host platform, with no extra configuration needed. To build for a
different target (cross-compiling), set:

- `OUISYNC_TARGET` to one of the triples above.
- `OUISYNC_CARGO`, if the host can't build that triple natively (e.g. linux aarch64 from an x86_64
  host), to a cargo-compatible binary that can, such as [`cross`](https://github.com/cross-rs/cross).
  Defaults to plain `cargo`.

On an unsupported host platform (with `OUISYNC_TARGET` unset) the hook skips bundling with a warning,
and `pip install`/`pip install -e .` still works, just without a bundled library (see "Tests" below).

The linux wheels use plain `linux_x86_64`/`linux_aarch64` tags rather than `manylinux_*`/`musllinux_*`,
so they aren't guaranteed to run on every glibc/musl system -- only ones close enough to the build
environment (currently ubuntu-24.04).

At runtime, `ouisync.service` looks for the native library in this order:

1. The `OUISYNC_LIB` environment variable, if set, naming a specific file.
2. The bundled copy next to the installed package, if there is one.
3. The bare library name (`libouisync_service.so`/`.dylib`/
   `ouisync_service.dll`), relying on the platform's normal shared library
   search path.

## Tests

```
cargo build --package ouisync-service --lib
cd bindings/python/service
pip install -e . -e ../session
pytest
```

`tests/conftest.py` builds the `ouisync-service` cdylib itself (like the
`cargo build` step above) and points `OUISYNC_LIB` at it, so the manual
build isn't strictly necessary, it just avoids paying for it inside the
first test.
