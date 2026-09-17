"""Builds the `ouisync_service` cdylib once per test session and points the
bindings at it, mirroring `bindings/python/session/tests/conftest.py`
(which builds and spawns the `ouisync` binary the same way) and the
`cargoBuildUnitTest`/`jna.library.path` setup in
`bindings/kotlin/ouisync-session/build.gradle`."""

import os
import platform
import subprocess
from pathlib import Path

import pytest

WORKSPACE_ROOT = Path(__file__).resolve().parents[4]


def _lib_filename() -> str:
    system = platform.system()
    if system == "Linux":
        return "libouisync_service.so"
    elif system == "Darwin":
        return "libouisync_service.dylib"
    elif system == "Windows":
        return "ouisync_service.dll"
    else:
        raise RuntimeError(f"unsupported platform {system!r}")


@pytest.fixture(scope="session", autouse=True)
def ouisync_service_lib() -> Path:
    subprocess.run(
        ["cargo", "build", "--package", "ouisync-service", "--lib"],
        cwd=WORKSPACE_ROOT,
        check=True,
    )

    path = WORKSPACE_ROOT / "target" / "debug" / _lib_filename()
    os.environ["OUISYNC_LIB"] = str(path)
    return path
