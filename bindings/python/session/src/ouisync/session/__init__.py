"""Session API: connects to a running Ouisync service's local control socket.

Includes the bindgen-generated data types, `Request`/`Response` tagged unions
and `Session`/`Repository`/`File` API classes, plus the parts bindgen doesn't
generate: connect/close, and the streaming endpoints (handled by hand in
every other binding too)."""

import typing

from ._generated.api import (
    AccessMode,
    ErrorCode,
    File,
    NetworkEvent,
    NetworkSocket,
    NetworkStream,
    OuisyncError,
    Repository,
    Request_SessionSubscribeToNetwork,
    Response_NetworkEvent,
    Response_Unit,
    Session,
    dispatch_error,
)
from .client import Client
from .state_monitor import MonitorId

__all__ = [
    "AccessMode",
    "Client",
    "ErrorCode",
    "File",
    "MonitorId",
    "NetworkSocket",
    "NetworkStream",
    "OuisyncError",
    "Repository",
    "Session",
    "close",
    "connect",
    "dispatch_error",
    "subscribe_to_network_events",
]


async def connect(config_dir, host: str = "127.0.0.1") -> Session:
    client = await Client.connect(config_dir, host)
    return Session(client)


async def close(session: Session):
    await session._client.close()


async def subscribe_to_network_events(session: Session) -> typing.AsyncIterator[NetworkEvent]:
    async for response in session._client.subscribe(Request_SessionSubscribeToNetwork()):
        if isinstance(response, Response_NetworkEvent):
            yield response.value
        elif isinstance(response, Response_Unit):
            yield NetworkEvent.PEER_SET_CHANGE
