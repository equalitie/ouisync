"""In-process embedding of the Ouisync service"""

import asyncio
import ctypes

from ouisync.session import ErrorCode, dispatch_error

from ._bindings import StatusCallback, bindings

__all__ = ["Service", "init_log"]


class Service:
    """Manages the repositories and runs the sync protocol. It can be
    interacted with using a [Session][ouisync.session.Session] connected to
    the same config directory.

    Note: to create a service, use `Service.start`.
    """

    def __init__(self, handle: int):
        self._handle: int | None = handle

    @classmethod
    async def start(cls, config_path, debug_label: str | None = None) -> "Service":
        """Starts the service.

        `config_path` is the path to the config directory of this service.
        If it doesn't exist, it's created automatically. The service requires
        both read and write access to it.

        `debug_label` is an optional label used to distinguish multiple
        services running in the same process. Used mainly for testing and
        debugging the library itself.
        """
        error_code, handle = await _invoke(
            bindings().start_service,
            str(config_path).encode(),
            debug_label.encode() if debug_label is not None else None,
        )

        if error_code != ErrorCode.OK:
            raise dispatch_error(error_code)

        return cls(handle)

    async def stop(self):
        """Stops this service. Has no effect if the service has already been
        stopped."""
        handle = self._handle
        if handle is None:
            return

        self._handle = None

        error_code, _ = await _invoke(bindings().stop_service, handle)

        if error_code != ErrorCode.OK:
            raise dispatch_error(error_code)


async def _invoke(func, *args):
    """Calls one of the `start_service`/`stop_service` native functions,
    appending the status callback and its (unused) context, and awaits its
    completion. Returns `(error_code, return_value)`."""
    loop = asyncio.get_running_loop()
    future: asyncio.Future = loop.create_future()

    # The callback may be invoked from a thread other than this one, so it
    # must hand the result back to the event loop thread-safely. Kept alive
    # by this frame for as long as the native side may still call it.
    @StatusCallback
    def callback(_context: ctypes.c_void_p, error_code: int):
        loop.call_soon_threadsafe(_resolve, future, error_code)

    return_value = func(*args, callback, None)
    error_code = ErrorCode(await future)

    return error_code, return_value


def _resolve(future: asyncio.Future, error_code: int):
    if not future.done():
        future.set_result(error_code)


def init_log():
    """Enables logging of Ouisync's internal messages.

    Calling this function more than once has no effect. Currently there is
    no way to disable the logging once it's been enabled.
    """
    bindings().init_log()
