#!/usr/bin/env bash
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo run --package ouisync-bindgen -- swift \
    > bindings/swift/OuisyncLib/Sources/generated/Api.swift
