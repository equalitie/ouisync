"""Session API: connects to a running Ouisync service's local control socket.

Includes the bindgen-generated data types, `Request`/`Response` tagged unions
and `Session`/`Repository`/`File` API classes, plus the parts bindgen doesn't
generate: connect/close, and the streaming endpoints (handled by hand in
every other binding too)."""

import inspect
import typing

from ._generated import api as _api
from ._generated.api import *
from .client import Client as _Client
from .state_monitor import MonitorId


async def connect(config_dir, host: str = "127.0.0.1") -> Session:
    client = await _Client.connect(config_dir, host)
    return Session(client)


async def close(session: Session):
    await session._client.close()


async def subscribe_to_network_events(session: Session) -> typing.AsyncIterator[NetworkEvent]:
    async for response in session._client.subscribe(Request_SessionSubscribeToNetwork()):
        if isinstance(response, Response_NetworkEvent):
            yield response.value
        elif isinstance(response, Response_Unit):
            yield NetworkEvent.PEER_SET_CHANGE

# Re-export everything from the generated api module.
_all = []

for name, member in inspect.getmembers(_api):
    if not inspect.isclass(member):
        continue

    # Don't re-export private clases
    if name.startswith('_'):
        continue

    # Don't re-export "handle" classes as they are used only internally by the API classes (which
    # are re-exported).
    if name.endswith('Handle'):
        continue

    _all.append(name)

# Re-export handwritten classes
_all.append('MonitorId')

__all__ = _all
