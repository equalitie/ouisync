# Multi-Directory Synchronization - Integration Tests

This directory contains integration tests for the multi-directory synchronization feature.

## Test Files

- **`share_integration_test.rs`** - Tests for share lifecycle management (13 tests)
- **`conflict_integration_test.rs`** - Tests for conflict detection and resolution (13 tests)

## Running Tests

### Prerequisites

Make sure you have Rust and Cargo installed:
```bash
rustc --version
cargo --version
```

### Run All Integration Tests

```bash
# From the ouisync project root
cargo test --package ouisync-service --test '*'
```

### Run Specific Test File

```bash
# Share integration tests
cargo test --package ouisync-service --test share_integration_test

# Conflict integration tests
cargo test --package ouisync-service --test conflict_integration_test
```

### Run Individual Test

```bash
# Run a specific test by name
cargo test --package ouisync-service --test share_integration_test test_add_share

# Run with debug output
cargo test --package ouisync-service --test share_integration_test test_add_share -- --nocapture
```

### Run with Logging

```bash
# Enable tracing/logging during tests
RUST_LOG=debug cargo test --package ouisync-service --test share_integration_test -- --nocapture
```

## Test Structure

Each test file follows this pattern:

```rust
// Helper functions
async fn create_test_service() -> (Service, TempDir, TempDir) { ... }
async fn create_test_directory(parent: &TempDir, name: &str) -> PathBuf { ... }

// Test functions
#[tokio::test]
async fn test_some_feature() {
    // Arrange: Set up test environment
    let (service, _config_dir, _store_dir) = create_test_service().await;
    
    // Act: Perform the operation
    let result = service.state().some_operation().await;
    
    // Assert: Verify the outcome
    assert!(result.is_ok());
}
```

## What's Tested

### Share Management (`share_integration_test.rs`)
- ✅ Adding shares to directories
- ✅ Removing shares (with/without repo deletion)
- ✅ Listing shares
- ✅ Getting share information
- ✅ Managing peers (add/remove/list)
- ✅ Enabling/disabling sync
- ✅ Generating share tokens
- ✅ Configuration persistence across restarts
- ✅ Error handling (duplicate shares, invalid paths, etc.)

### Conflict Resolution (`conflict_integration_test.rs`)
- ✅ Detecting file modification conflicts
- ✅ Detecting file creation conflicts
- ✅ Detecting file deletion conflicts
- ✅ Detecting directory-file conflicts
- ✅ Resolving conflicts with KeepLocal strategy
- ✅ Resolving conflicts with KeepRemote strategy
- ✅ Resolving conflicts with KeepBoth strategy
- ✅ Resolving conflicts with DeleteBoth strategy
- ✅ Conflict window behavior (10-second threshold)
- ✅ Handling multiple conflicts
- ✅ Error handling (non-existent conflicts)

## Test Isolation

All tests use:
- Temporary directories via `tempfile::TempDir`
- Isolated service instances
- No shared state between tests
- Automatic cleanup after each test

## Troubleshooting

### Test Failures

If tests fail, check:
1. All dependencies are installed (`cargo build --package ouisync-service`)
2. No other service instances are running
3. Sufficient disk space for temporary files
4. Proper file permissions

### Timeout Issues

Some tests involve asynchronous operations with delays. If tests timeout:
- Increase the timeout with: `cargo test -- --test-threads=1`
- Check system resources (CPU, disk I/O)

### Platform-Specific Issues

Some tests may behave differently on:
- **Linux**: File watching uses inotify
- **macOS**: File watching uses FSEvents
- **Windows**: File watching uses ReadDirectoryChangesW

## Adding New Tests

When adding tests:
1. Use descriptive test names: `test_<feature>_<scenario>`
2. Include comments explaining what's being tested
3. Use helper functions to reduce duplication
4. Test both success and error cases
5. Ensure tests are isolated and can run in any order

Example:
```rust
#[tokio::test]
async fn test_new_feature() {
    // Arrange
    let (service, _config, _store) = create_test_service().await;
    
    // Act
    let result = service.state().new_feature().await;
    
    // Assert
    assert!(result.is_ok(), "New feature should work");
}
```

## Related Documentation

- `../../docs/multi_dir_sync_test_plan.md` - Comprehensive test plan
- `../../docs/phase_4_3_testing_summary.md` - Testing phase summary
- `../../docs/multi_dir_sync.md` - Feature specification
- `../../docs/conflict_resolution.md` - Conflict resolution design

## Continuous Integration

These tests are designed to run in CI/CD environments:
- No interactive prompts
- No external dependencies
- Deterministic outcomes
- Fast execution (<5 minutes for full suite)

## Coverage

To generate code coverage reports:
```bash
# Install tarpaulin
cargo install cargo-tarpaulin

# Generate coverage
cargo tarpaulin --package ouisync-service --tests --out Html
```

## Questions?

For questions or issues:
1. Check the test plan: `docs/multi_dir_sync_test_plan.md`
2. Review the implementation: `service/src/share_api.rs`
3. Check existing GitHub issues
4. Ask the development team

---

**Last Updated**: Phase 4.3 - Comprehensive Testing
