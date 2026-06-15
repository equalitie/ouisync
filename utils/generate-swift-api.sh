#!/usr/bin/env bash
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo run --package ouisync-bindgen -- swift 2>/dev/null \
    > bindings/swift/OuisyncLib/Sources/generated/Api.swift
