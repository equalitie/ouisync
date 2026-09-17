"""
Exercises the exact RPC call surface paskoocheh-web's ouisync_sync/client.py
uses, end to end against a real daemon -- the parity check from the design
plan, run for real rather than just cross-referenced by name.
"""

from ouisync.session import AccessMode, connect, close


async def test_session_protocol_version(daemon):
    config_dir, _store_dir = daemon
    session = await connect(str(config_dir))
    try:
        version = await session.get_current_protocol_version()
        assert isinstance(version, int)
    finally:
        await close(session)


async def test_repository_lifecycle_and_file_roundtrip(daemon, tmp_path):
    config_dir, store_dir = daemon
    session = await connect(str(config_dir))
    try:
        await session.set_store_dirs(paths=[str(store_dir)])

        repo = await session.create_repository(path=str(tmp_path / "repo"))
        assert repo is not None

        token = await repo.share(access_mode=AccessMode.WRITE)
        assert isinstance(token, str)

        await repo.set_dht_enabled(enabled=False)
        await repo.set_pex_enabled(enabled=False)

        info_hash = await repo.get_info_hash()
        assert isinstance(info_hash, str)

        await repo.create_directory(path="/docs")
        entries = await repo.read_directory(path="/docs")
        assert list(entries) == []

        assert await repo.file_exists(path="/docs/hello.txt") is False

        content = b"hello ouisync"
        handle = await repo.create_file(path="/docs/hello.txt")
        await handle.write(offset=0, data=content)
        await handle.close()

        assert await repo.file_exists(path="/docs/hello.txt") is True

        opened = await repo.open_file(path="/docs/hello.txt")
        read_back = await opened.read(offset=0, size=len(content))
        await opened.close()
        assert read_back == content

        stats = await repo.get_stats()
        assert stats is not None

        await repo.remove_file(path="/docs/hello.txt")
        assert await repo.file_exists(path="/docs/hello.txt") is False

        await repo.delete()
    finally:
        await close(session)


async def test_session_find_and_list_repositories(daemon, tmp_path):
    config_dir, store_dir = daemon
    session = await connect(str(config_dir))
    try:
        await session.set_store_dirs(paths=[str(store_dir)])

        path = str(tmp_path / "repo2")
        await session.create_repository(path=path)

        repos = await session.list_repositories()
        assert isinstance(repos, dict)
        assert len(repos) >= 1

        found = await session.find_repository(name=path)
        assert found is not None
    finally:
        await close(session)
