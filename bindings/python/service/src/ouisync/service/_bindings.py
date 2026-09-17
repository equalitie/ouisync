"""ctypes declarations for the `ouisync_service` native library"""

import ctypes
import os
import platform
from pathlib import Path

# Callback invoked by `start_service`/`stop_service` once the corresponding
# operation completes. First argument is the `callback_context` passed in
# unchanged, second is the `ErrorCode` (as a raw `u16`).
StatusCallback = ctypes.CFUNCTYPE(None, ctypes.c_void_p, ctypes.c_uint16)


class Bindings:
    def __init__(self, lib: ctypes.CDLL):
        self.start_service = lib.start_service
        self.start_service.argtypes = [
            ctypes.c_char_p,
            ctypes.c_char_p,
            StatusCallback,
            ctypes.c_void_p,
        ]
        self.start_service.restype = ctypes.c_void_p

        self.stop_service = lib.stop_service
        self.stop_service.argtypes = [
            ctypes.c_void_p,
            StatusCallback,
            ctypes.c_void_p,
        ]
        self.stop_service.restype = None

        self.init_log = lib.init_log
        self.init_log.argtypes = []
        self.init_log.restype = None


def _default_library_path() -> str:
    if "OUISYNC_LIB" in os.environ:
        return os.environ["OUISYNC_LIB"]

    name = "ouisync_service"
    system = platform.system()
    if system == "Linux":
        filename = f"lib{name}.so"
    elif system == "Darwin":
        filename = f"lib{name}.dylib"
    elif system == "Windows":
        filename = f"{name}.dll"
    else:
        raise RuntimeError(f"unsupported platform {system!r}")

    # `hatch_build.py` bundles a native library built for the current platform
    # right next to this package -- prefer it over relying on the system's
    # shared library search path.
    bundled = Path(__file__).resolve().parent / "_native" / filename
    if bundled.is_file():
        return str(bundled)

    return filename


_instance: Bindings | None = None


def bindings() -> Bindings:
    """Returns the process-wide `Bindings` instance, loading the native
    library on first use."""
    global _instance
    if _instance is None:
        _instance = Bindings(ctypes.CDLL(_default_library_path()))
    return _instance
