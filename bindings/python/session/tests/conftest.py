"""Starts an in-process `ouisync-service` per test, mirroring
`bindings/python/service/tests/conftest.py`'s own use of `Service` (and,
before this, the way `cli/tests/utils.rs` spawns a real `ouisync` binary for
the Rust project's tests)."""

import os
import platform
import subprocess
from pathlib import Path

import pytest

from ouisync.service import Service

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


@pytest.fixture
async def daemon(tmp_path):
    config_dir = tmp_path / "config"
    store_dir = tmp_path / "store"
    config_dir.mkdir()
    store_dir.mkdir()

    service = await Service.start(str(config_dir))
    try:
        yield config_dir, store_dir
    finally:
        await service.stop()
