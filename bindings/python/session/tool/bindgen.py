#!/usr/bin/env python3
"""Regenerates src/ouisync/session/_generated/api.py from the Rust API surface.

Run from anywhere; invokes `cargo run --package ouisync-bindgen -- python`
from the workspace root and writes its stdout to the generated file, the
same pattern bindings/dart/tool/bindgen.dart and the Kotlin `generateApi`
Gradle task use for their own languages.
"""

import subprocess
from pathlib import Path

WORKSPACE_ROOT = Path(__file__).resolve().parents[4]
OUT_FILE = Path(__file__).resolve().parents[1] / "src" / "ouisync" / "session" / "_generated" / "api.py"


def main() -> None:
    result = subprocess.run(
        ["cargo", "run", "--package", "ouisync-bindgen", "--", "python"],
        cwd=WORKSPACE_ROOT,
        capture_output=True,
        text=True,
        check=True,
    )

    OUT_FILE.parent.mkdir(parents=True, exist_ok=True)
    OUT_FILE.write_text(result.stdout)
    print(f"wrote {OUT_FILE}")


if __name__ == "__main__":
    main()
