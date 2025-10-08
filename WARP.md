# WARP.md

This file provides guidance to WARP (warp.dev) when working with code in this repository.

## Project Overview

Ouisync is a secure peer-to-peer file syncing library and application written in Rust. The project consists of a core library with language bindings (Dart, Kotlin, Swift) and multiple applications including a CLI and service components.

## Common Commands

### Building the Project

Build the entire workspace:
```bash
cargo build
```

Build for release:
```bash
cargo build --release
```

Build specific components:
```bash
# Build the CLI
cargo build --release --bin ouisync

# Build the service library (shared library for bindings)
cargo build -p ouisync-service --lib
```

### Testing

Run all workspace tests:
```bash
cargo test --workspace --lib
```

Run specific test suites:
```bash
# Library unit tests
cargo test --workspace --lib

# Core library integration tests
cargo test --package ouisync --test gc --test network --test sync

# CLI integration tests (Linux only)
cargo test --package ouisync-cli --test cli
```

Run tests with debug logging:
```bash
RUST_LOG=ouisync=debug,gc=debug,network=debug,sync=debug cargo test --package ouisync --test network
```

### Linting and Code Quality

Run clippy on main packages:
```bash
cargo clippy --package ouisync --package ouisync-cli --package ouisync-net --package ouisync-service --package ouisync-vfs --package fs_util --package logtee --all-targets --no-deps -- -Dwarnings
```

### Cross-compilation

For Android targets (requires cross):
```bash
cargo install cross --force --git https://github.com/cross-rs/cross
cross build --target aarch64-linux-android
```

### Language Bindings

Test Kotlin bindings:
```bash
cd bindings/kotlin
./gradlew --stacktrace testDebugUnitTest
```

Test Dart bindings:
```bash
cd bindings/dart
flutter pub get
dart run tool/bindgen.dart
flutter test
```

### Running the CLI

After building, the CLI can be found at `target/release/ouisync`. Basic usage:
```bash
./target/release/ouisync --help
./target/release/ouisync start
```

## Architecture Overview

### Workspace Structure

This is a Cargo workspace with the following key crates:

- **`lib/`** - Core Ouisync library (`ouisync` crate)
- **`cli/`** - Command-line interface (`ouisync-cli` crate)
- **`service/`** - Service layer with C-compatible API (`ouisync-service` crate)
- **`vfs/`** - Virtual filesystem implementation (`ouisync-vfs` crate)
- **`net/`** - Network layer (`ouisync-net` crate)
- **`bindings/`** - Language bindings (Dart, Kotlin, Swift)

### Core Library Components

The main library (`lib/`) is organized around these key concepts:

- **Repository** - Core data structure representing a synced folder
- **Access Control** - Permission management and share tokens
- **Crypto** - Cryptographic primitives (hashing, signing, encryption)
- **Network** - P2P networking and peer discovery
- **VFS** - Virtual file system for mounting repositories
- **Sync** - Conflict-free synchronization between peers
- **Protocol** - Wire protocol for peer communication

Key modules:
- `repository/` - Repository management and operations
- `access_control/` - Share tokens and permission handling
- `crypto/` - Cryptographic operations
- `network/` - P2P networking, DHT, peer discovery
- `directory/` - Directory and file operations
- `sync/` - Synchronization algorithm
- `protocol/` - Network protocol definitions

### Service Layer

The `service/` crate provides:
- C-compatible FFI API for language bindings
- WebSocket API for GUI applications
- HTTP API for remote management
- Cross-platform service daemon functionality

### Networking

Ouisync uses multiple discovery mechanisms:
- **Local Discovery** - mDNS/broadcast for LAN peers
- **DHT** - Distributed hash table for internet peer discovery
- **PEX** - Peer exchange protocol
- **Manual peers** - Explicitly configured peer addresses

Supported protocols: TCP and QUIC over IPv4/IPv6

### Testing Strategy

The project uses multiple testing approaches:
- Unit tests in individual modules
- Integration tests in `lib/tests/`
- CLI integration tests
- Language binding tests
- Simulation tests using the `turmoil` framework
- Property-based testing with `proptest`

### Key Dependencies

- **tokio** - Async runtime
- **sqlx** - Database (SQLite) operations
- **rustls** - TLS implementation
- **ed25519-dalek** - Digital signatures
- **blake3** - Cryptographic hashing
- **btdht** - BitTorrent DHT implementation

### Development Environment

System dependencies for development:
- Linux: `libfuse-dev`
- Windows: Dokany2 (`choco install dokany2`)
- Android: NDK version specified in CI (currently 28.2.13676358)

The project targets Rust 1.87.0+ and uses edition 2024.

### Configuration Files

- `Cargo.toml` - Workspace configuration
- `Cross.toml` - Cross-compilation settings
- `.github/workflows/ci.yml` - CI pipeline configuration