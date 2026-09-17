"""Spawns a real `ouisync` daemon binary as a subprocess per test, mirroring
cli/tests/utils.rs's `Bin::start()`."""

import subprocess
import time
from pathlib import Path

import pytest

WORKSPACE_ROOT = Path(__file__).resolve().parents[4]


@pytest.fixture(scope="session")
def ouisync_binary() -> Path:
    # Default `vfs` feature pulls in xpc-connection, broken on current toolchains.
    subprocess.run(
        ["cargo", "build", "--bin", "ouisync", "--no-default-features"],
        cwd=WORKSPACE_ROOT,
        check=True,
    )
    return WORKSPACE_ROOT / "target" / "debug" / "ouisync"


@pytest.fixture
def daemon(tmp_path, ouisync_binary):
    config_dir = tmp_path / "config"
    store_dir = tmp_path / "store"
    config_dir.mkdir()
    store_dir.mkdir()

    process = subprocess.Popen(
        [str(ouisync_binary), "--config-dir", str(config_dir), "start"],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )

    conf_path = config_dir / "local_endpoint.conf"
    deadline = time.monotonic() + 30

    while not conf_path.exists():
        if process.poll() is not None:
            stdout = process.stdout.read() if process.stdout else ""
            stderr = process.stderr.read() if process.stderr else ""
            raise RuntimeError(f"ouisync exited early:\nstdout: {stdout}\nstderr: {stderr}")

        if time.monotonic() > deadline:
            process.terminate()
            raise TimeoutError("local_endpoint.conf did not appear in time")

        time.sleep(0.05)

    try:
        yield config_dir, store_dir
    finally:
        process.terminate()

        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
