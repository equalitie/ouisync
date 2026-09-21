"""Builds the ouisync-service cdylib and bundles it into the wheel, so
installing this package doesn't require a separate Rust build step (unlike
the editable/test setup in tests/conftest.py, which builds its own debug
copy and points OUISYNC_LIB at it instead).

By default this builds for the host platform. To cross-compile for a
different platform, set OUISYNC_TARGET to one of the Rust target
triples in _TARGETS below, and (if the host can't build that triple
natively, e.g. linux aarch64 from an x86_64 host) OUISYNC_CARGO to
"cross" instead of the default "cargo".

Each wheel is built for exactly one target -- one wheel per entry in
_TARGETS, no fat/universal wheels. Linux wheels use plain `linux_*` tags
rather than `manylinux_*`/`musllinux_*`, so they aren't guaranteed to work
on every glibc/musl system, only ones close enough to the build environment
(currently ubuntu-24.04).
"""

from __future__ import annotations

import os
import platform
import subprocess
from pathlib import Path
from typing import Any

from hatchling.builders.hooks.plugin.interface import BuildHookInterface

# Rust target triple -> (native library filename, wheel platform tag).
_TARGETS = {
    "x86_64-unknown-linux-gnu":  ("libouisync_service.so",      "linux_x86_64"),
    "aarch64-unknown-linux-gnu": ("libouisync_service.so",      "linux_aarch64"),
    "x86_64-pc-windows-msvc":    ("ouisync_service.dll",        "win_amd64"),
    "x86_64-pc-windows-gnu":     ("ouisync_service.dll",        "win_amd64"),
    "aarch64-pc-windows-msvc":   ("ouisync_service.dll",        "win_arm64"),
    "aarch64-pc-windows-gnu":    ("ouisync_service.dll",        "win_arm64"),
    "x86_64-apple-darwin":       ("libouisync_service.dylib",   "macosx_10_12_x86_64"),
    "aarch64-apple-darwin":      ("libouisync_service.dylib",   "macosx_11_0_arm64"),
}

# Host (platform.system(), platform.machine()) -> its native Rust target triple.
# Used when OUISYNC_SERVICE_TARGET isn't set, so a plain `pip install -e .` on a
# supported host still works without any extra configuration.
_NATIVE_TARGETS = {
    ("Linux",   "x86_64"):  "x86_64-unknown-linux-gnu",
    ("Linux",   "aarch64"): "aarch64-unknown-linux-gnu",
    ("Windows", "AMD64"):   "x86_64-pc-windows-msvc",
    ("Windows", "ARM64"):   "aarch64-pc-windows-msvc",
    ("Darwin",  "x86_64"):  "x86_64-apple-darwin",
    ("Darwin",  "arm64"):   "aarch64-apple-darwin",
}


class CargoBuildHook(BuildHookInterface):
    def initialize(self, version: str, build_data: dict[str, Any]) -> None:
        if self.target_name != "wheel":
            return

        host = (platform.system(), platform.machine())
        host_target_triple = _NATIVE_TARGETS.get(host)

        target_triple = os.environ.get("OUISYNC_TARGET")

        if target_triple is None:
            target_triple = host_target_triple

            if target_triple is None:
                supported = ", ".join(f"{system} {machine}" for system, machine in _NATIVE_TARGETS)
                self.app.display_warning(
                    f"not bundling ouisync_service: unsupported host platform {host[0]} {host[1]} "
                    f"(supported: {supported}; or set OUISYNC_TARGET explicitly)"
                )
                return

        target = _TARGETS.get(target_triple)
        if target is None:
            supported = ", ".join(_TARGETS)
            raise ValueError(
                f"unsupported OUISYNC_TARGET {target_triple!r} (supported: {supported})"
            )

        lib_filename, wheel_tag = target
        cargo_bin = os.environ.get("OUISYNC_CARGO", "cargo")

        # self.root is bindings/python/service; the workspace root is three levels up.
        workspace_root = Path(self.root).resolve().parents[2]

        args = [
            cargo_bin,
            "build",
            "--release",
            "--package",
            "ouisync-service",
            "--lib",
        ]
        lib_path = ""

        if target_triple == host_target_triple and cargo_bin == "cargo":
            lib_path = workspace_root / "target" / "release" / lib_filename
        else:
            lib_path = workspace_root / "target" / target_triple / "release" / lib_filename
            args.append("--target")
            args.append(target_triple)

        subprocess.run(args, cwd=workspace_root, check=True)

        build_data["force_include"][str(lib_path)] = f"ouisync/service/_native/{lib_filename}"

        # The wheel now contains a platform-specific native library.
        build_data["pure_python"] = False
        build_data["tag"] = f"py3-none-{wheel_tag}"
