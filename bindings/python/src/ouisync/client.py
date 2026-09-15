"""Client for an Ouisync daemon's local control socket."""

import asyncio
import dataclasses
import hashlib
import hmac
import json
import secrets
import struct
import types
import typing
from pathlib import Path

import msgpack

from ._generated.api import (
    ErrorCode,
    OuisyncError,
    Response,
    Response_None,
    dispatch_error,
)

AUTH_CHALLENGE_SIZE = 256
AUTH_PROOF_SIZE = 32  # SHA-256 output size

# Sentinel put on a subscription's queue to signal a clean end of stream --
# distinct from `None` since `None` is itself a valid decoded value.
_STREAM_DONE = object()


class Client:
    """Client for one Ouisync daemon's local control-socket protocol."""

    def __init__(self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter):
        self._reader = reader
        self._writer = writer
        self._next_id = 0
        self._pending: dict[int, asyncio.Future] = {}
        self._subscriptions: dict[int, asyncio.Queue] = {}
        self._receive_task: asyncio.Task | None = None

    @classmethod
    async def connect(cls, config_dir, host: str = "127.0.0.1") -> "Client":
        # host is explicit since the control socket can be bound non-loopback.
        conf_path = Path(config_dir) / "local_endpoint.conf"
        data = json.loads(conf_path.read_text())
        auth_key = bytes.fromhex(data["auth_key"])

        reader, writer = await asyncio.open_connection(host, data["port"])
        client = cls(reader, writer)

        try:
            await client._auth(auth_key)
        except Exception:
            writer.close()
            raise

        client._receive_task = asyncio.create_task(client._receive_loop())
        return client

    async def _auth(self, auth_key: bytes):
        client_challenge = secrets.token_bytes(AUTH_CHALLENGE_SIZE)
        self._writer.write(client_challenge)
        await self._writer.drain()

        server_proof = await self._recv_exact(AUTH_PROOF_SIZE)
        server_challenge = await self._recv_exact(AUTH_CHALLENGE_SIZE)

        expected_proof = hmac.new(auth_key, client_challenge, hashlib.sha256).digest()
        if not hmac.compare_digest(server_proof, expected_proof):
            raise OuisyncError(ErrorCode.PERMISSION_DENIED, "server proof did not match")

        client_proof = hmac.new(auth_key, server_challenge, hashlib.sha256).digest()
        self._writer.write(client_proof)
        await self._writer.drain()

    async def _recv_exact(self, n: int) -> bytes:
        try:
            return await self._reader.readexactly(n)
        except asyncio.IncompleteReadError as error:
            raise ConnectionError("socket closed while reading") from error

    async def _send_frame(self, payload: bytes):
        self._writer.write(struct.pack(">I", len(payload)) + payload)
        await self._writer.drain()

    async def _recv_frame(self) -> bytes:
        (length,) = struct.unpack(">I", await self._recv_exact(4))
        return await self._recv_exact(length)

    async def invoke(self, request) -> "Response":
        message_id = self._next_id
        self._next_id += 1

        future: asyncio.Future = asyncio.get_running_loop().create_future()
        self._pending[message_id] = future

        try:
            await self._send_message(message_id, request)
        except Exception:
            self._pending.pop(message_id, None)
            raise

        return await future

    async def subscribe(self, request) -> typing.AsyncIterator["Response"]:
        message_id = self._next_id
        self._next_id += 1

        queue: asyncio.Queue = asyncio.Queue()
        self._subscriptions[message_id] = queue

        try:
            await self._send_message(message_id, request)

            while True:
                item = await queue.get()

                if item is _STREAM_DONE:
                    return

                if isinstance(item, Exception):
                    raise item

                yield item
        finally:
            self._subscriptions.pop(message_id, None)

    async def _send_message(self, message_id: int, request):
        payload = struct.pack(">Q", message_id) + msgpack.packb(
            encode_value(request), use_bin_type=True
        )
        await self._send_frame(payload)

    async def close(self):
        if self._receive_task is not None:
            self._receive_task.cancel()

        self._writer.close()

        try:
            await self._writer.wait_closed()
        except Exception:
            pass

    async def _receive_loop(self):
        try:
            while True:
                frame = await self._recv_frame()
                message_id = struct.unpack(">Q", frame[:8])[0]
                raw = msgpack.unpackb(frame[8:], raw=False, strict_map_key=False)

                try:
                    result = _decode_response_result(raw)
                except Exception as error:  # deliver decode failures to the waiter
                    result = error

                self._dispatch(message_id, result)
        except (asyncio.IncompleteReadError, ConnectionError, asyncio.CancelledError):
            pass
        finally:
            self._fail_all(ConnectionError("connection closed"))

    def _dispatch(self, message_id: int, result):
        future = self._pending.pop(message_id, None)
        if future is not None:
            if not future.done():
                if isinstance(result, Exception):
                    future.set_exception(result)
                else:
                    future.set_result(result)
            return

        queue = self._subscriptions.get(message_id)
        if queue is None:
            # Response for a subscription that was already cancelled, or a
            # message id we don't recognize -- nothing to deliver it to.
            return

        if isinstance(result, Exception):
            queue.put_nowait(result)
        elif isinstance(result, Response_None):
            queue.put_nowait(_STREAM_DONE)
        else:
            queue.put_nowait(result)

    def _fail_all(self, error: Exception):
        for future in self._pending.values():
            if not future.done():
                future.set_exception(error)
        self._pending.clear()

        for queue in self._subscriptions.values():
            queue.put_nowait(error)
        self._subscriptions.clear()


def _decode_response_result(raw):
    if isinstance(raw, dict) and "Failure" in raw:
        code, message, sources = raw["Failure"]
        return dispatch_error(ErrorCode(code), message, sources)

    if isinstance(raw, dict) and "Success" in raw:
        return decode_value(raw["Success"], Response)

    return OuisyncError(ErrorCode.OTHER, f"unrecognized response shape: {raw!r}")


# --- generic codec for the generated dataclasses/tagged unions ---

_type_hints_cache: dict[type, dict[str, typing.Any]] = {}


def _type_hints(cls: type) -> dict[str, typing.Any]:
    hints = _type_hints_cache.get(cls)
    if hints is None:
        hints = typing.get_type_hints(cls)
        _type_hints_cache[cls] = hints
    return hints


def encode_value(value):
    if value is None or isinstance(value, (bool, int, float, str, bytes)):
        return value

    if isinstance(value, list):
        return [encode_value(v) for v in value]

    if isinstance(value, dict):
        return {encode_value(k): encode_value(v) for k, v in value.items()}

    if dataclasses.is_dataclass(value):
        cls = type(value)
        shape = getattr(cls, "_shape", "named")
        tag = getattr(cls, "_tag", None)

        if shape == "unit":
            inner = None
        elif shape == "unnamed":
            inner = encode_value(value.value)
        else:
            inner = [encode_value(getattr(value, f.name)) for f in dataclasses.fields(value)]

        if tag is not None:
            return tag if shape == "unit" else {tag: inner}

        if shape == "unit":
            return []

        return inner

    raise TypeError(f"don't know how to encode {value!r}")


def decode_value(raw, target: typing.Any):
    if target is typing.Any or target is type(None):
        return raw

    if target in (int, str, bool, float, bytes):
        return raw

    origin = typing.get_origin(target)

    if origin is list:
        (item_type,) = typing.get_args(target)
        return [decode_value(v, item_type) for v in raw]

    if origin is dict:
        key_type, value_type = typing.get_args(target)
        return {decode_value(k, key_type): decode_value(v, value_type) for k, v in raw.items()}

    if origin is types.UnionType or origin is typing.Union:
        (inner_type,) = (a for a in typing.get_args(target) if a is not type(None))
        return None if raw is None else decode_value(raw, inner_type)

    if isinstance(target, type) and hasattr(target, "__members__"):  # IntEnum subclass
        return target(raw)

    if isinstance(target, type) and hasattr(target, "_variants"):
        return _decode_tagged_union(raw, target)

    if isinstance(target, type) and dataclasses.is_dataclass(target):
        return _decode_struct(raw, target)

    raise TypeError(f"don't know how to decode into {target!r}")


def _decode_tagged_union(raw, base_cls: type):
    if isinstance(raw, str):
        tag, inner = raw, None
    else:
        ((tag, inner),) = raw.items()

    variant_cls = base_cls._variants[tag]
    return _construct(variant_cls, inner)


def _decode_struct(raw, cls: type):
    return _construct(cls, raw)


def _construct(cls: type, inner):
    shape = cls._shape
    hints = _type_hints(cls)

    if shape == "unit":
        return cls()

    if shape == "unnamed":
        return cls(decode_value(inner, hints["value"]))

    fields = dataclasses.fields(cls)
    values = [decode_value(v, hints[f.name]) for v, f in zip(inner, fields)]
    return cls(*values)
