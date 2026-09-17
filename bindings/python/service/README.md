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

## Loading the native library

By default the `ouisync_service` shared library is looked up by its bare
name (`libouisync_service.so`/`.dylib`/`ouisync_service.dll`), relying on
the platform's normal shared library search path. Set the `OUISYNC_LIB`
environment variable to point at a specific file instead.

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
