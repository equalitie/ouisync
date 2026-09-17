"""Builds the ouisync-service cdylib for the current platform and bundles
it into the wheel, so installing this package doesn't require a separate
Rust build step (unlike the editable/test setup in tests/conftest.py,
which builds its own debug copy and points OUISYNC_LIB at it instead).

Each wheel is built natively on a matching CI runner (see the
build_python_service_package matrix in .github/workflows/ci.yml) -- one
wheel per entry in _TARGETS below, no cross-compilation. Linux wheels use
plain `linux_*` tags rather than `manylinux_*`/`musllinux_*`, so they
aren't guaranteed to work on every glibc/musl system, only ones close
enough to the build environment (currently ubuntu-24.04).
"""

from __future__ import annotations

import platform
import subprocess
from pathlib import Path
from typing import Any

from hatchling.builders.hooks.plugin.interface import BuildHookInterface

# (platform.system(), platform.machine()) -> (native library filename, wheel platform tag)
_TARGETS = {
    ("Linux", "x86_64"): ("libouisync_service.so", "linux_x86_64"),
    ("Windows", "AMD64"): ("ouisync_service.dll", "win_amd64"),
}


class CargoBuildHook(BuildHookInterface):
    def initialize(self, version: str, build_data: dict[str, Any]) -> None:
        if self.target_name != "wheel":
            return

        key = (platform.system(), platform.machine())
        target = _TARGETS.get(key)
        if target is None:
            supported = ", ".join(f"{system} {machine}" for system, machine in _TARGETS)
            self.app.display_warning(
                f"not bundling ouisync_service: unsupported platform {key[0]} {key[1]} "
                f"(supported: {supported})"
            )
            return

        lib_filename, wheel_tag = target

        # self.root is bindings/python/service; the workspace root is three levels up.
        workspace_root = Path(self.root).resolve().parents[2]

        subprocess.run(
            ["cargo", "build", "--release", "--package", "ouisync-service", "--lib"],
            cwd=workspace_root,
            check=True,
        )

        lib_path = workspace_root / "target" / "release" / lib_filename
        build_data["force_include"][str(lib_path)] = f"ouisync/service/_native/{lib_filename}"

        # The wheel now contains a platform-specific native library.
        build_data["pure_python"] = False
        build_data["tag"] = f"py3-none-{wheel_tag}"
