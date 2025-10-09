# Multi-Directory Synchronization Implementation Guide

## Overview

This document describes the implementation of multi-directory synchronization in Ouisync, which transforms it from a virtual filesystem sync tool into a real filesystem directory sync tool (similar to Syncthing or Resilio Sync).

## Architecture Summary

### Core Concept: "Shares"

A **Share** is a regular filesystem directory that Ouisync monitors and synchronizes with peers:
- Users add directories like `/home/user/Documents` or `/home/user/Photos`
- Each share gets its own `.ouisyncdb` repository file (stored inside the directory)
- Bidirectional real-time sync: `Local FS ↔ Ouisync DB ↔ P2P Network`
- Each share has its own manual peer list
- One directory = one share (one-to-one mapping)

## Implementation Details

### Phase 1: Core Components (✅ Completed)

#### 1.1 Share Data Structures (`service/src/share.rs`)

**`ShareInfo`** - Public representation of a share:
```rust
pub struct ShareInfo {
    pub id: Uuid,                    // Unique identifier
    pub path: PathBuf,               // Absolute local filesystem path
    pub name: String,                // User-friendly name
    pub peers: Vec<PeerAddr>,        // Manual peer list
    pub sync_enabled: bool,          // Is sync active?
    pub repository_id: RepositoryId, // Underlying Ouisync repository ID
}
```

**`ShareHolder`** - Internal holder for resources:
```rust
pub(crate) struct ShareHolder {
    info: ShareInfo,
    repository: Arc<Repository>,     // Ouisync repository
    sync_bridge: Option<SyncBridge>, // Bidirectional sync component
}
```

**`ShareSet`** - Thread-safe collection of all shares:
- Indexed by UUID, path, and handle
- Provides concurrent access with `RwLock`
- Similar to existing `RepositorySet` structure

#### 1.2 Bidirectional Sync Bridge (`service/src/sync_bridge.rs`)

The sync bridge is the core of the bidirectional synchronization:

**Filesystem → Repository:**
- Uses `notify` crate for cross-platform file watching
- Monitors for file/directory create, modify, delete events
- Ignores hidden files and `.ouisyncdb` 
- Writes changes to Ouisync repository

**Repository → Filesystem:**
- Subscribes to repository change events
- Syncs repository contents to filesystem
- Handles conflicts (later phase)
- Batch changes with small delay to avoid thrashing

**Key Features:**
- Initial sync on startup (repo → fs)
- Real-time incremental syncing
- Recursive directory handling
- Efficient change detection

#### 1.3 Integration into Service State (`service/src/state.rs`)

Added `ShareSet` to the `State` struct:
```rust
pub(crate) struct State {
    // ... existing fields ...
    shares: ShareSet,
    // ...
}
```

### Phase 2: Public API (✅ Completed)

All Share Management APIs are implemented in `service/src/share_api.rs`:

#### Core APIs

**`share_add(path: PathBuf) -> Result<Uuid, Error>`**
- Creates a new share from a filesystem directory
- Validates path exists and is absolute
- Creates `.ouisyncdb` repository inside directory
- Initializes sync bridge
- Registers with network
- Returns share UUID

**`share_remove(share_id: Uuid, delete_repo_file: bool) -> Result<(), Error>`**
- Stops syncing
- Removes share from active set
- Optionally deletes `.ouisyncdb` file
- Keeps actual files in directory intact

**`share_list() -> Vec<ShareInfo>`**
- Lists all configured shares with their status

**`share_get_info(share_id: Uuid) -> Result<ShareInfo, Error>`**
- Gets detailed info about a specific share

#### Peer Management APIs

**`share_add_peer(share_id: Uuid, peer_addr: PeerAddr) -> Result<(), Error>`**
- Adds a peer to the share's manual peer list
- Peer address format: `"quic/192.168.1.100:20209"` or `"tcp/..."`

**`share_remove_peer(share_id: Uuid, peer_addr: PeerAddr) -> Result<(), Error>`**
- Removes a peer from the share

**`share_list_peers(share_id: Uuid) -> Result<Vec<PeerAddr>, Error>`**
- Lists all peers for a share

#### Control APIs

**`share_set_sync_enabled(share_id: Uuid, enabled: bool) -> Result<(), Error>`**
- Enable/disable syncing for a share

**`share_get_sync_token(share_id: Uuid, access_mode: AccessMode) -> Result<String, Error>`**
- Generates a share token for other peers to connect
- Supports Read, Write, or Blind access modes
- Token can be shared via QR code or text

## Dependencies Added

### `service/Cargo.toml`
```toml
uuid = { version = "1.11", features = ["v4", "serde"] }
notify = { version = "6.1", default-features = false, features = ["macos_fsevent"] }
```

## Usage Example

### From Rust/Service Layer

```rust
// Initialize service
let service = Service::init(config_dir).await?;
let state = service.state();

// Add a directory as a share
let path = PathBuf::from("/home/user/Documents");
let share_id = state.share_add(path).await?;

// Add a peer to sync with
let peer = "quic/192.168.1.100:20209".parse()?;
state.share_add_peer(share_id, peer)?;

// Get sync token to share with other peers
let token = state.share_get_sync_token(share_id, AccessMode::Write)?;
println!("Share this token: {}", token);

// List all shares
let shares = state.share_list();
for share in shares {
    println!("{}: {} - {} peers", share.name, share.path.display(), share.peers.len());
}

// Remove a share (keeps files, removes sync)
state.share_remove(share_id, false).await?;
```

### From Language Bindings (Next Phase)

Once bindings are updated, usage from Kotlin/Dart/Swift will look like:

```kotlin
// Kotlin
val shareId = client.shareAdd("/path/to/directory")
client.shareAddPeer(shareId, "quic/192.168.1.100:20209")
val token = client.shareGetSyncToken(shareId, AccessMode.WRITE)
```

```dart
// Dart
final shareId = await client.shareAdd('/path/to/directory');
await client.shareAddPeer(shareId, 'quic/192.168.1.100:20209');
final token = await client.shareGetSyncToken(shareId, AccessMode.write);
```

## Next Steps (TODO)

### Phase 2.2: Language Bindings (Not Started)

Need to update FFI exports and bindings for Kotlin, Dart, and Swift:

1. **FFI Exports** (`service/src/ffi.rs`):
   - Export share management functions with C-compatible signatures
   - Handle string/UUID marshaling

2. **Kotlin Bindings** (`bindings/kotlin/`):
   - Add share management methods to `OuisyncClient`
   - Create `ShareInfo` data class
   - Handle serialization

3. **Dart Bindings** (`bindings/dart/`):
   - Update `tool/bindgen.dart` to generate share APIs
   - Add share management methods
   - Create `ShareInfo` class

4. **Swift Bindings** (`bindings/swift/`):
   - Similar updates for Swift

### Phase 4: Conflict Resolution & Testing

#### 4.1 Conflict Handling (Not Started)

Current implementation does basic sync. Need to add:

**Conflict Detection:**
- Detect when same file modified on filesystem and from network
- Use timestamps/hashes to identify conflicts

**Conflict Resolution:**
- Rename conflicted version: `filename.sync-conflict-YYYYMMDD-HHMMSS.ext`
- Keep both versions
- Add API to list conflicts: `share_list_conflicts(share_id)`
- Add API to resolve: `share_resolve_conflict(share_id, conflict_id, resolution)`

**Example Implementation:**
```rust
#[api]
pub async fn share_list_conflicts(&self, share_id: Uuid) -> Result<Vec<ConflictInfo>, Error> {
    // Scan for .sync-conflict-* files
    // Return list of conflicts with metadata
}

#[api]
pub async fn share_resolve_conflict(
    &self,
    share_id: Uuid,
    conflict_id: Uuid,
    resolution: ConflictResolution, // KeepLocal, KeepRemote, KeepBoth, Delete
) -> Result<(), Error> {
    // Apply resolution strategy
}
```

#### 4.2 Configuration Persistence (Not Started)

Currently shares are only in memory. Need to:

1. **Create Share Config File** (`~/.config/ouisync/shares.json`):
```json
{
  "shares": [
    {
      "id": "uuid",
      "path": "/home/user/Documents",
      "name": "Documents",
      "peers": ["quic/..."],
      "sync_enabled": true
    }
  ]
}
```

2. **Load Shares on Startup:**
   - In `State::init()`, load share config
   - Re-create shares and sync bridges

3. **Persist Changes:**
   - Save config after add/remove/modify operations
   - Atomic write with temp file + rename

#### 4.3 Testing (Not Started)

**Unit Tests:**
- `share.rs`: Test ShareSet operations
- `sync_bridge.rs`: Test sync logic with mock filesystem

**Integration Tests:**
- Create share, modify files, verify sync
- Test peer communication
- Test conflict scenarios

**End-to-End Tests:**
- Multi-device sync scenarios
- Network interruption recovery
- Large file handling

## Build and Compilation

### Prerequisites

```bash
# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Install system dependencies
# Linux:
sudo apt install libfuse-dev pkg-config

# macOS:
brew install macfuse

# Windows:
choco install dokany2
```

### Building

```bash
# Check compilation
cargo check --package ouisync-service

# Build
cargo build --package ouisync-service

# Build release
cargo build --release --package ouisync-service

# Run tests
cargo test --package ouisync-service --lib
```

## Architecture Diagrams

### Component Interaction

```
┌─────────────────┐
│   GUI/Frontend  │
│ (External Repo) │
└────────┬────────┘
         │
         │ Language Bindings (Kotlin/Dart/Swift)
         │
┌────────▼────────────────────────────────────┐
│         Service Layer (Rust)                │
│                                             │
│  ┌──────────────┐      ┌─────────────────┐ │
│  │ Share API    │◄─────┤  State          │ │
│  │ (share_api)  │      │  - ShareSet     │ │
│  └──────┬───────┘      │  - RepositorySet│ │
│         │              │  - Network      │ │
│         │              └─────────────────┘ │
│  ┌──────▼───────┐                          │
│  │ ShareHolder  │                          │
│  │  - info      │                          │
│  │  - repository│◄─────┐                   │
│  │  - bridge    │      │                   │
│  └──────┬───────┘      │                   │
│         │              │                   │
│  ┌──────▼───────────┐  │                   │
│  │  SyncBridge      │  │                   │
│  │  - FileWatcher   │  │                   │
│  │  - RepoEvents    │  │                   │
│  └──────┬───────────┘  │                   │
│         │              │                   │
└─────────┼──────────────┼───────────────────┘
          │              │
          │              │
┌─────────▼──────┐  ┌────▼─────────────┐
│  Filesystem    │  │  Repository      │
│  /home/user/   │◄─┤  (.ouisyncdb)    │
│  Documents/    │  │                  │
└────────────────┘  └──────────────────┘
                           │
                           │ P2P Network
                           │
                    ┌──────▼──────┐
                    │   Peers     │
                    └─────────────┘
```

### Data Flow: Adding a File

```
1. User creates file.txt in /home/user/Documents/

2. SyncBridge detects via FileWatcher
   ↓
3. Reads file content
   ↓
4. Writes to Repository (.ouisyncdb)
   ↓
5. Repository propagates to Network
   ↓
6. Network syncs to connected Peers
   ↓
7. Peer's Repository receives change
   ↓
8. Peer's SyncBridge detects repo change
   ↓
9. Writes file to peer's filesystem
```

## Breaking Changes

This implementation **does not maintain backward compatibility** with the existing Ouisync workflow:

**Old Model:**
- Repositories stored in central `store_dir`
- Repositories mounted via VFS to view files
- Global peer discovery (DHT/PEX)

**New Model:**
- Shares are regular directories with embedded `.ouisyncdb`
- Direct filesystem access (no mounting needed)
- Manual peer lists per share

**Migration Path:**
If you need to support old repositories, you could:
1. Add a migration function to convert old repos to shares
2. Maintain both code paths (complex)
3. Provide conversion tool

## Known Limitations

1. **No DHT/PEX:** Currently only manual peers. Could add later if needed.
2. **Simple Conflict Resolution:** Renames conflicts, no merge strategies.
3. **No Configuration Persistence:** Shares don't survive restart yet.
4. **No Progress Reporting:** No way to see sync status/progress.
5. **No Bandwidth Control:** No QoS or rate limiting.

## Performance Considerations

1. **File Watching Overhead:** Using `notify` crate, which is efficient but still has CPU cost
2. **Large Files:** Current implementation reads entire files into memory
3. **Many Small Files:** Each change triggers individual sync operation
4. **Network Efficiency:** Uses Ouisync's existing efficient block-level sync

## Security Considerations

1. **Repository Encryption:** Each `.ouisyncdb` uses encrypted storage
2. **Network Encryption:** P2P connections use TLS (QUIC/TCP+TLS)
3. **Access Control:** Share tokens control read/write/blind access
4. **Local Access:** `.ouisyncdb` file should have appropriate permissions

## Debugging

Enable debug logging:
```bash
RUST_LOG=ouisync_service=debug,ouisync=debug cargo run
```

Watch sync bridge activity:
```bash
RUST_LOG=ouisync_service::sync_bridge=trace cargo run
```

## Contributing

When contributing to this feature:
1. Update this document with any architectural changes
2. Add tests for new functionality
3. Update language bindings consistently
4. Consider backward compatibility impact

## References

- Ouisync Core Library: `lib/`
- Service Layer: `service/`
- Language Bindings: `bindings/`
- Original PRD: (see project documentation)
