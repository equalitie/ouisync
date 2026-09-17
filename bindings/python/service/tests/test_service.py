"""Lifecycle tests for the in-process service, mirroring the Rust
`sanity_check` test in `service/src/ffi.rs` and the `Service` usage in
`bindings/kotlin/ouisync-session/.../SyncTest.kt`."""

from ouisync.service import Service, init_log
from ouisync.session import close, connect


async def test_start_stop(tmp_path):
    init_log()

    service = await Service.start(str(tmp_path / "config"))
    await service.stop()


async def test_session_connects_to_started_service(tmp_path):
    config_dir = tmp_path / "config"

    service = await Service.start(str(config_dir))
    try:
        session = await connect(str(config_dir))
        try:
            version = await session.get_current_protocol_version()
            assert isinstance(version, int)
        finally:
            await close(session)
    finally:
        await service.stop()
