"""Builds the ouisync-service cdylib and bundles it into the wheel, so
installing this package doesn't require a separate Rust build step (unlike
the editable/test setup in tests/conftest.py, which builds its own debug
copy and points OUISYNC_LIB at it instead).

Linux x86_64 only for now.
"""

from __future__ import annotations

import platform
import subprocess
from pathlib import Path
from typing import Any

from hatchling.builders.hooks.plugin.interface import BuildHookInterface

_LIB_NAME = "libouisync_service.so"


class CargoBuildHook(BuildHookInterface):
    def initialize(self, version: str, build_data: dict[str, Any]) -> None:
        if self.target_name != "wheel":
            return

        system, machine = platform.system(), platform.machine()
        if system != "Linux" or machine not in ("x86_64", "AMD64"):
            self.app.display_warning(
                f"not bundling ouisync_service: unsupported platform {system} {machine} "
                "(only linux x86_64 is supported for now)"
            )
            return

        # self.root is bindings/python/service; the workspace root is three levels up.
        workspace_root = Path(self.root).resolve().parents[2]

        subprocess.run(
            ["cargo", "build", "--release", "--package", "ouisync-service", "--lib"],
            cwd=workspace_root,
            check=True,
        )

        lib_path = workspace_root / "target" / "release" / _LIB_NAME
        build_data["force_include"][str(lib_path)] = f"ouisync/service/_native/{_LIB_NAME}"

        # The wheel now contains a platform-specific native library.
        build_data["pure_python"] = False
        build_data["tag"] = "py3-none-linux_x86_64"
