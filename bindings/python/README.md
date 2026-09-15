# ouisync (Python binding)

A generated Python binding for Ouisync, following the same pattern as
`bindings/dart`, `bindings/kotlin`, `bindings/cpp`, and `bindings/swift`:
`utils/bindgen` reads the `#[api]`-annotated Rust source and emits the data
types, `Request`/`Response` tagged unions, and `Session`/`Repository`/`File`
API classes into `src/ouisync/_generated/api.py`; a hand-written
`src/ouisync/client.py` implements the actual socket transport (auth
handshake, framing) and a small generic MessagePack codec that the
generated dataclasses plug into.

Unlike Dart/Kotlin/Swift, this binding is socket-only: it connects to an
already-running external `ouisync` process (no in-process FFI embedding).
It's also plain `asyncio`, matching every other binding's async
concurrency model.

## Regenerating the API surface

```
python tool/bindgen.py
```

Runs `cargo run --package ouisync-bindgen -- python` from the workspace
root and writes its output to `src/ouisync/_generated/api.py`. The
generated file is checked into git (like `bindings/cpp`/`bindings/swift`)
so installing this package never requires a Rust toolchain.

## Usage

```python
import ouisync

session = await ouisync.connect("/path/to/config")
repo = await session.create_repository("/path/to/store")
token = await repo.share()
```

## Tests

```
cd bindings/python
pip install -e .
pytest
```

Tests spawn a real `ouisync` binary as a subprocess per test (see
`tests/conftest.py`), the same way `cli/tests/utils.rs` does for the Rust
project's own tests -- no Docker, no mocks.
