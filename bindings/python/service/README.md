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
build step. Each wheel is platform-specific, built natively (no cross-compilation) for one of:

- linux x86_64
- Windows x86_64

(macOS and non-x86_64 targets aren't built yet.)

`.github/workflows/ci.yml`'s `build_python_service_package` job runs one matrix leg per target and
uploads the resulting wheels. On any other platform the hook skips bundling with a warning, and
`pip install`/`pip install -e .` still works, just without a bundled library (see "Tests" below).

The linux wheel uses a plain `linux_x86_64` tag rather than `manylinux_*`/`musllinux_*`, so it isn't
guaranteed to run on every glibc/musl system -- only ones close enough to the CI build environment
(currently ubuntu-24.04).

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
