"""Session API parts bindgen doesn't generate: connect/close, and the
streaming endpoints (handled by hand in every other binding too)."""

import typing

from ._generated.api import (
    NetworkEvent,
    Request_SessionSubscribeToNetwork,
    Response_NetworkEvent,
    Response_Unit,
    Session,
)
from .client import Client


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
