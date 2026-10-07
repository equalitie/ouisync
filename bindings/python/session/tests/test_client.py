import json
import secrets

from ouisync.service import Service
from ouisync.session import Request_SessionGetStoreDirs, Response_Paths
from ouisync.session.client import Client


# Run sanity check with the default API protocol transport (unix domain socket on platforms that
# support it, TCP on loopback otherwise)
async def test_sanity_check_default(tmp_path):
    await _sanity_check(tmp_path / "config")


# Run sanity check with TCP on loopback as the API protocol transport.
async def test_sanity_check_tcp(tmp_path):
    config_dir = tmp_path / "config"
    config_dir.mkdir()

    addr = f"tcp://127.0.0.1:0?auth_key={secrets.token_hex(32)}"
    (config_dir / "local_endpoint.conf").write_text(json.dumps(addr))

    await _sanity_check(config_dir)


async def _sanity_check(config_dir):
    service = await Service.start(str(config_dir))
    try:
        client = await Client.connect(config_dir)
        try:
            response = await client.invoke(Request_SessionGetStoreDirs())
            assert isinstance(response, Response_Paths)
        finally:
            await client.close()
    finally:
        await service.stop()
