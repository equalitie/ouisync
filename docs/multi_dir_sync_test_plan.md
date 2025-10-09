# Multi-Directory Synchronization - Test Plan

This document outlines the comprehensive testing strategy for the multi-directory synchronization feature.

## Test Coverage Overview

### 1. Unit Tests

#### 1.1 Conflict Module (`service/src/conflict.rs`)
- ✅ Conflict type serialization/deserialization
- ✅ Conflict resolution strategy validation
- ✅ Conflict creation with proper IDs and timestamps
- ✅ Edge cases (invalid paths, null conflicts)

**Status**: 7 tests implemented

#### 1.2 Share Config Module (`service/src/share_config.rs`)
- Configuration serialization/deserialization
- File I/O operations (save/load)
- Atomic write operations
- Handling corrupt or missing config files
- Validation of duplicate share IDs

**Status**: To be implemented

#### 1.3 Sync Bridge Module (`service/src/sync_bridge.rs`)
- Bridge creation and initialization
- Repository opening with correct access modes
- Peer management (add/remove)
- Conflict window validation (10-second threshold)
- Handle concurrent modifications
- Thread safety and synchronization

**Status**: To be implemented

### 2. Integration Tests

#### 2.1 Share Management (`service/tests/share_integration_test.rs`)
- ✅ Add share to existing directory
- ✅ Add share to non-existent directory (error case)
- ✅ Add duplicate share (error case)
- ✅ Remove share (with and without repository deletion)
- ✅ List shares
- ✅ Get share info
- ✅ Share token generation
- ✅ Peer management (add/list/remove)
- ✅ Sync enable/disable
- ✅ Configuration persistence across service restarts
- ✅ Error handling for invalid operations

**Status**: 13 tests implemented

#### 2.2 Conflict Management (`service/tests/conflict_integration_test.rs`)
- ✅ Conflict detection (file modified, created, deleted)
- ✅ Directory-file conflicts
- ✅ Conflict resolution strategies (KeepLocal, KeepRemote, KeepBoth, DeleteBoth)
- ✅ Multiple conflicts handling
- ✅ Conflict window behavior (inside vs outside 10-second window)
- ✅ Empty conflict list
- ✅ Error handling for non-existent conflicts

**Status**: 13 tests implemented

#### 2.3 State Persistence (`service/tests/state_persistence_test.rs`)
- Configuration file creation
- Loading shares on service startup
- Saving shares on modification
- Handling corrupt configuration
- Migration from old config format (if applicable)

**Status**: To be implemented

### 3. End-to-End Tests

#### 3.1 Two-Peer Synchronization
Test scenario: Two services synchronizing a directory
- Setup two service instances with separate config/store directories
- Add same directory as share on both
- Exchange share tokens
- Verify initial sync
- Modify files on one peer, verify sync to other
- Test bidirectional sync
- Verify conflict detection and resolution

**Status**: To be implemented

#### 3.2 Multi-Peer Synchronization
Test scenario: Three or more peers synchronizing
- Setup multiple service instances
- Daisy-chain connections (A ↔ B ↔ C)
- Verify transitive synchronization
- Test concurrent modifications from multiple peers
- Verify conflict resolution propagates

**Status**: To be implemented

#### 3.3 Large File Synchronization
- Test synchronization of files > 100MB
- Test many small files (1000+)
- Test directory hierarchies (deep nesting)
- Verify performance and memory usage

**Status**: To be implemented

#### 3.4 Network Interruption Handling
- Simulate network disconnection during sync
- Verify graceful handling and resumption
- Test peer reconnection
- Verify no data corruption

**Status**: To be implemented

### 4. Stress Tests

#### 4.1 Concurrent Operations
- Multiple shares added/removed concurrently
- Rapid peer additions/removals
- High-frequency file modifications
- Stress test conflict detection with many simultaneous conflicts

**Status**: To be implemented

#### 4.2 Long-Running Tests
- Service running for extended periods (hours/days)
- Continuous synchronization activity
- Memory leak detection
- Resource usage monitoring

**Status**: To be implemented

### 5. Performance Tests

#### 5.1 Synchronization Performance
- Measure time to sync 1GB of data
- Measure time to sync 10,000 small files
- Benchmark different network conditions (LAN, WAN, high latency)
- Compare performance with/without encryption

**Status**: To be implemented

#### 5.2 Conflict Resolution Performance
- Measure conflict detection latency
- Measure resolution time for each strategy
- Test performance with many pending conflicts (100+)

**Status**: To be implemented

## Test Execution

### Running Unit Tests

```bash
# Run all service unit tests
cargo test --package ouisync-service --lib

# Run specific module tests
cargo test --package ouisync-service --lib conflict::tests
cargo test --package ouisync-service --lib sync_bridge::tests
```

### Running Integration Tests

```bash
# Run all integration tests
cargo test --package ouisync-service --test '*'

# Run specific integration test
cargo test --package ouisync-service --test share_integration_test
cargo test --package ouisync-service --test conflict_integration_test
```

### Running All Tests

```bash
# Run everything
cargo test --package ouisync-service --lib --tests
```

## Test Environment Requirements

- **OS**: Linux (Ubuntu/Pop!_OS), macOS, Windows
- **Rust**: 1.87.0+
- **Dependencies**: libfuse-dev (Linux), Dokany2 (Windows)
- **Disk Space**: At least 10GB free for test artifacts
- **Network**: Loopback interface must be available

## Test Data

Test data is managed using:
- `tempfile::TempDir` for temporary directories
- Automatically cleaned up after tests
- No persistent test data between runs

## Coverage Goals

- **Unit Test Coverage**: >80% of new code
- **Integration Test Coverage**: All major workflows
- **End-to-End Test Coverage**: Critical user scenarios
- **Edge Case Coverage**: All identified failure modes

## Known Limitations

1. **Network Simulation**: Current tests use localhost only; real network conditions not simulated
2. **Cross-Platform**: Some tests may be platform-specific (filesystem behavior)
3. **Performance Tests**: Not yet implemented; benchmarks needed
4. **Chaos Engineering**: No fault injection tests yet

## Future Test Enhancements

1. **Property-Based Testing**: Use `proptest` for conflict scenarios
2. **Fuzzing**: Fuzz test conflict resolution logic
3. **Cross-Platform CI**: Test on Linux, macOS, Windows in CI pipeline
4. **Network Simulation**: Use `comfy-table` or `turmoil` for network simulation
5. **Visual Testing**: Add tests for UI components (when implemented)
6. **Backward Compatibility**: Test migration from previous versions

## Test Maintenance

- Review and update tests when feature requirements change
- Add regression tests for every bug found
- Refactor tests to improve readability and maintainability
- Keep test execution time reasonable (<5 minutes for full suite)

## Test Metrics

Track the following metrics:
- Test execution time
- Test pass rate
- Code coverage percentage
- Number of tests per component
- Flaky test incidents

## Conclusion

This test plan ensures comprehensive coverage of the multi-directory synchronization feature. As development progresses, this document should be updated to reflect new tests and coverage improvements.

**Current Status**: Phase 4.3 (Comprehensive Testing) - ~30% complete
- Unit tests: 7 implemented
- Integration tests: 26 implemented
- E2E tests: 0 implemented
- Total: 33 tests
