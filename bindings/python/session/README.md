# ouisync-session (Python binding)

The session half of the Python binding: this package is the client, and `bindings/python/service` is
the in-process service embedding.

A generated Python binding for Ouisync: `utils/bindgen` reads the `#[api]`-annotated Rust source and
emits the data types and API classes into `src/ouisync/session/_generated/api.py`; a hand-written
`src/ouisync/session/client.py` implements the actual socket transport (auth handshake, framing) and
a small generic MessagePack codec that the generated dataclasses plug into.

This package is socket-only: it connects to an already-running Ouisync
service (started either by `bindings/python/service`, or by an external
`ouisync` process) over the local control socket. It's plain `asyncio`,
matching every other binding's async concurrency model.

## Regenerating the API surface

```
python tool/bindgen.py
```

Runs `cargo run --package ouisync-bindgen -- python` from the workspace
root and writes its output to `src/ouisync/session/_generated/api.py`. The
generated file is checked into git (like `bindings/cpp`/`bindings/swift`)
so installing this package never requires a Rust toolchain.

## Usage

```python
from ouisync.session import connect

session = await connect("/path/to/config")
repo = await session.create_repository("/path/to/store")
token = await repo.share()
```

## Tests

```
cd bindings/python/session
pip install -e . -e ../service
pytest
```

Tests start a real `ouisync-service` in-process per test (see
`tests/conftest.py`, which builds the `ouisync-service` cdylib itself and
points `OUISYNC_LIB` at it, mirroring `bindings/python/service/tests/conftest.py`)
and connect a `Session` to it -- no Docker, no mocks.
