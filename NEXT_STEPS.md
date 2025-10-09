# Next Steps - Multi-Directory Synchronization

## Quick Start for Continuing Development

### 1. Build and Test Current Implementation

```bash
# First, install Rust if not already installed
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source $HOME/.cargo/env

# Install system dependencies (Linux)
sudo apt install libfuse-dev pkg-config

# Check if everything compiles
cd /home/guid/projects/ouisync
cargo check --package ouisync-service

# Build the service
cargo build --package ouisync-service

# Run tests
cargo test --package ouisync-service --lib
```

### 2. Priority: Complete Configuration Persistence

**Why First:** Shares currently don't survive restart. This is critical for usability.

**Files to Create:**
- `service/src/share_config.rs` - Config persistence logic

**Implementation Steps:**
```rust
// 1. Define config structure
#[derive(Serialize, Deserialize)]
struct SharesConfig {
    shares: Vec<ShareConfigEntry>,
}

#[derive(Serialize, Deserialize)]
struct ShareConfigEntry {
    id: Uuid,
    path: PathBuf,
    name: String,
    peers: Vec<PeerAddr>,
    sync_enabled: bool,
}

// 2. Save to ~/.config/ouisync/shares.json
async fn save_shares_config(shares: &[ShareInfo]) -> Result<()> {
    let config = SharesConfig { shares: shares.into() };
    let json = serde_json::to_string_pretty(&config)?;
    // Atomic write: temp file + rename
    fs::write(temp_path, json).await?;
    fs::rename(temp_path, config_path).await?;
    Ok(())
}

// 3. Load on State::init()
async fn load_shares_config() -> Result<Vec<ShareInfo>> {
    let json = fs::read_to_string(config_path).await?;
    let config: SharesConfig = serde_json::from_str(&json)?;
    Ok(config.shares)
}

// 4. Call save_shares_config() after each modification in share_api.rs
```

**Integration Points:**
- Modify `State::init()` in `state.rs` to call `load_shares_config()`
- Add `save_shares_config()` calls in `share_api.rs` after add/remove/modify

**Testing:**
```bash
# Add share
# Restart service
# Verify share still exists
```

### 3. Next: Language Bindings (Phase 2.2)

**Start With:** Dart bindings (simplest, used by GUI)

**Files to Modify:**
1. `service/src/ffi.rs` - Add FFI exports
2. `bindings/dart/lib/ouisync.dart` - Add share methods
3. `bindings/dart/tool/bindgen.dart` - Generate bindings

**Example FFI Export:**
```rust
// In service/src/ffi.rs
#[no_mangle]
pub extern "C" fn ouisync_share_add(
    state: *const State,
    path: *const c_char,
    out_uuid: *mut u8, // 16 bytes for UUID
) -> c_int {
    // Implement C-compatible wrapper
}
```

**Example Dart Binding:**
```dart
// In bindings/dart/lib/ouisync.dart
class OuisyncClient {
  Future<String> shareAdd(String path) async {
    // Call FFI function
    // Return UUID as string
  }
  
  Future<void> shareAddPeer(String shareId, String peerAddr) async {
    // ...
  }
}
```

### 4. Then: Conflict Resolution (Phase 4.1)

**Files to Modify:**
- `service/src/sync_bridge.rs` - Add conflict detection
- `service/src/share_api.rs` - Add conflict APIs

**Implementation:**
```rust
// Detect conflicts
async fn handle_fs_event(...) {
    // Before writing to repo, check if repo has newer version
    let repo_modified = repo.get_file_modified_time(path).await?;
    let fs_modified = fs::metadata(path).await?.modified()?;
    
    if repo_modified > fs_modified {
        // Conflict! Rename file
        let conflict_name = format!(
            "{}.sync-conflict-{}.{}",
            stem,
            chrono::Utc::now().format("%Y%m%d-%H%M%S"),
            ext
        );
        fs::rename(path, conflict_path).await?;
    }
}

// List conflicts API
#[api]
pub fn share_list_conflicts(&self, share_id: Uuid) -> Result<Vec<ConflictInfo>> {
    // Scan directory for .sync-conflict-* files
}
```

### 5. Finally: Comprehensive Testing (Phase 4.2)

**Test Structure:**
```
service/tests/
  share_test.rs      - Unit tests for ShareSet
  sync_bridge_test.rs - Sync bridge tests  
  integration_test.rs - Full workflow tests
```

**Example Test:**
```rust
#[tokio::test]
async fn test_bidirectional_sync() {
    // Setup two peers
    let peer1 = setup_peer("/tmp/peer1").await?;
    let peer2 = setup_peer("/tmp/peer2").await?;
    
    // Add share on peer1
    let share_id = peer1.share_add("/tmp/peer1/test").await?;
    
    // Connect peers
    peer1.share_add_peer(share_id, peer2.address()).await?;
    
    // Create file on peer1
    fs::write("/tmp/peer1/test/file.txt", "hello").await?;
    
    // Wait for sync
    tokio::time::sleep(Duration::from_secs(5)).await;
    
    // Verify file exists on peer2
    let content = fs::read_to_string("/tmp/peer2/test/file.txt").await?;
    assert_eq!(content, "hello");
}
```

## File Structure Summary

### New Files (Already Created):
```
service/src/
  share.rs          ✅ - Share data structures
  sync_bridge.rs    ✅ - Bidirectional sync
  share_api.rs      ✅ - Public API
  
docs/
  MULTI_DIR_SYNC_IMPLEMENTATION.md  ✅ - Architecture guide
  IMPLEMENTATION_SUMMARY.md          ✅ - What's done
  NEXT_STEPS.md                      ✅ - This file
```

### Files to Create (TODO):
```
service/src/
  share_config.rs   ⏳ - Config persistence
  
service/tests/
  share_test.rs     ⏳ - Unit tests
  sync_bridge_test.rs ⏳ - Sync tests
  integration_test.rs ⏳ - Integration tests
  
bindings/*/
  (various)         ⏳ - Language binding updates
```

### Files to Modify (TODO):
```
service/src/
  ffi.rs            ⏳ - Add FFI exports
  state.rs          ⏳ - Load shares on init
  
bindings/dart/
  lib/ouisync.dart  ⏳ - Add share methods
  tool/bindgen.dart ⏳ - Generate bindings
  
bindings/kotlin/
  (various)         ⏳ - Add share methods
  
bindings/swift/
  (various)         ⏳ - Add share methods
```

## Common Issues and Solutions

### Issue: Compilation Errors

**Problem:** Rust not installed or wrong version
```bash
# Solution
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup update
rustc --version  # Should be 1.87.0+
```

**Problem:** Missing system dependencies
```bash
# Solution (Linux)
sudo apt install libfuse-dev pkg-config build-essential

# Solution (macOS)
brew install macfuse pkg-config

# Solution (Windows)
choco install dokany2
```

### Issue: Share Not Syncing

**Debug Steps:**
```bash
# 1. Enable debug logging
RUST_LOG=ouisync_service=debug,ouisync=debug cargo run

# 2. Check if sync bridge started
# Look for: "Sync bridge task started"

# 3. Check if file watcher working
# Create file and look for: "File created/modified: ..."

# 4. Check if network connected
# Look for: "Connected to peer: ..."

# 5. Check repository events
# Look for: "Repository changed, syncing to filesystem"
```

### Issue: Files Not Appearing on Peer

**Checklist:**
1. ✓ Both peers have same share ID?
2. ✓ Peer addresses correctly added?
3. ✓ Network connectivity (ping peer)?
4. ✓ Share tokens match?
5. ✓ Sync enabled on both sides?
6. ✓ No firewall blocking?

## Development Workflow

### Daily Development:
```bash
# 1. Pull latest changes
git pull origin main

# 2. Make changes to source files
vim service/src/share_config.rs

# 3. Check compilation
cargo check --package ouisync-service

# 4. Run tests
cargo test --package ouisync-service

# 5. Build
cargo build --package ouisync-service

# 6. Test manually if needed
RUST_LOG=debug cargo run

# 7. Commit
git add .
git commit -m "Add share config persistence"
git push origin feature/multi-dir-sync
```

### Before Pull Request:
```bash
# 1. Run all tests
cargo test --workspace --lib

# 2. Run clippy
cargo clippy --package ouisync --package ouisync-service --all-targets --no-deps -- -Dwarnings

# 3. Format code
cargo fmt --all

# 4. Update documentation
vim IMPLEMENTATION_SUMMARY.md

# 5. Create PR with clear description
```

## API Usage Examples (For Testing)

### Rust (Service Layer):
```rust
use ouisync_service::{Service, Error};
use ouisync::{AccessMode, PeerAddr};
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<(), Error> {
    // Initialize service
    let mut service = Service::init(PathBuf::from("/tmp/ouisync-config")).await?;
    let state = service.state();
    
    // Add a share
    let share_id = state.share_add(PathBuf::from("/tmp/test-share")).await?;
    println!("Share created: {}", share_id);
    
    // Add a peer
    let peer: PeerAddr = "quic/127.0.0.1:20209".parse()?;
    state.share_add_peer(share_id, peer)?;
    
    // Get share token
    let token = state.share_get_sync_token(share_id, AccessMode::Write)?;
    println!("Share token: {}", token);
    
    // List all shares
    for share in state.share_list() {
        println!("Share: {} at {:?}", share.name, share.path);
    }
    
    // Keep service running
    service.run().await?;
}
```

### Dart (Once Bindings Updated):
```dart
import 'package:ouisync/ouisync.dart';

Future<void> main() async {
  final client = OuisyncClient();
  await client.init('/tmp/ouisync-config');
  
  // Add a share
  final shareId = await client.shareAdd('/tmp/test-share');
  print('Share created: $shareId');
  
  // Add a peer
  await client.shareAddPeer(shareId, 'quic/127.0.0.1:20209');
  
  // Get share token
  final token = await client.shareGetSyncToken(shareId, AccessMode.write);
  print('Share token: $token');
  
  // List all shares
  final shares = await client.shareList();
  for (final share in shares) {
    print('Share: ${share.name} at ${share.path}');
  }
}
```

## Resources

### Documentation:
- **Architecture:** `MULTI_DIR_SYNC_IMPLEMENTATION.md`
- **Summary:** `IMPLEMENTATION_SUMMARY.md`
- **Project Docs:** `WARP.md`
- **This File:** `NEXT_STEPS.md`

### Code Locations:
- **Share Management:** `service/src/share.rs`
- **Sync Bridge:** `service/src/sync_bridge.rs`
- **Public API:** `service/src/share_api.rs`
- **State Integration:** `service/src/state.rs`

### External Resources:
- **Ouisync Repo:** https://github.com/equalitie/ouisync
- **Rust Async Book:** https://rust-lang.github.io/async-book/
- **Tokio Docs:** https://tokio.rs/
- **Notify Crate:** https://docs.rs/notify/

## Questions?

If you have questions:
1. Check `MULTI_DIR_SYNC_IMPLEMENTATION.md` for architecture
2. Check `IMPLEMENTATION_SUMMARY.md` for what's done
3. Read inline comments in source files
4. Check existing Ouisync documentation in `README.md`

## Timeline Estimate

- **Config Persistence:** 1-2 days
- **Dart Bindings:** 2-3 days  
- **Kotlin Bindings:** 2-3 days
- **Swift Bindings:** 2-3 days
- **Conflict Resolution:** 2-3 days
- **Testing:** 3-5 days

**Total:** ~12-20 days for complete implementation

**Current Progress:** ~60% (Phases 1 and 2.1 complete)

---

Good luck with the implementation! The foundation is solid. 🚀
