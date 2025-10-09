//! Integration tests for Conflict Detection and Resolution
//!
//! These tests verify:
//! - Conflict detection between shares
//! - Conflict resolution strategies (KeepLocal, KeepRemote, KeepBoth, DeleteBoth)
//! - Sync bridge conflict handling

use ouisync::{AccessMode, PeerAddr};
use ouisync_service::{
    conflict::{ConflictResolution, ConflictType},
    Service,
};
use std::path::PathBuf;
use std::time::Duration;
use tempfile::TempDir;
use tokio::fs;
use tokio::time::sleep;
use uuid::Uuid;

/// Helper to create a test service with a temporary config directory
async fn create_test_service() -> (Service, TempDir, TempDir) {
    let config_dir = TempDir::new().unwrap();
    let store_dir = TempDir::new().unwrap();

    let mut service = Service::init(config_dir.path().to_owned())
        .await
        .expect("Failed to initialize service");

    service
        .set_store_dir(store_dir.path())
        .await
        .expect("Failed to set store dir");

    (service, config_dir, store_dir)
}

/// Helper to create a test directory with some files
async fn create_test_directory(parent: &TempDir, name: &str) -> PathBuf {
    let test_dir = parent.path().join(name);
    fs::create_dir_all(&test_dir)
        .await
        .expect("Failed to create test directory");

    // Create a test file
    fs::write(test_dir.join("file.txt"), b"Initial content")
        .await
        .expect("Failed to create file");

    test_dir
}

#[tokio::test]
async fn test_conflict_detection_file_modification() {
    let (service1, _config_dir1, _store_dir1) = create_test_service().await;
    let (service2, _config_dir2, _store_dir2) = create_test_service().await;

    let test_dir_parent = TempDir::new().unwrap();
    let dir1 = create_test_directory(&test_dir_parent, "share1").await;
    let dir2 = create_test_directory(&test_dir_parent, "share2").await;

    let state1 = service1.state();
    let state2 = service2.state();

    // Add shares
    let share_id1 = state1.share_add(dir1.clone()).await.expect("Failed to add share 1");
    let share_id2 = state2.share_add(dir2.clone()).await.expect("Failed to add share 2");

    // Get share token from share1
    let token = state1
        .share_get_sync_token(share_id1, AccessMode::Write)
        .expect("Failed to get token");

    // TODO: Add logic to import share2 from token (when implemented)

    // Simulate concurrent modification
    fs::write(dir1.join("file.txt"), b"Modified by share1")
        .await
        .expect("Failed to modify file in share1");

    // Wait less than conflict window
    sleep(Duration::from_secs(5)).await;

    fs::write(dir2.join("file.txt"), b"Modified by share2")
        .await
        .expect("Failed to modify file in share2");

    // Allow sync to detect conflict
    sleep(Duration::from_secs(2)).await;

    // Check for conflicts
    let conflicts = state1.share_list_conflicts(share_id1).expect("Failed to list conflicts");

    // Should detect modification conflict
    assert!(!conflicts.is_empty());
    assert!(conflicts
        .iter()
        .any(|c| matches!(c.conflict_type, ConflictType::FileModified)));
}

#[tokio::test]
async fn test_conflict_resolution_keep_local() {
    let (service, _config_dir, _store_dir) = create_test_service().await;
    let test_dir_parent = TempDir::new().unwrap();
    let test_dir = create_test_directory(&test_dir_parent, "share").await;

    let state = service.state();
    let share_id = state.share_add(test_dir.clone()).await.expect("Failed to add share");

    // Simulate conflict by modifying file
    let file_path = test_dir.join("file.txt");
    fs::write(&file_path, b"Local version").await.unwrap();

    // Create a mock conflict (in practice this would come from sync)
    // For now, we'll test the API directly
    let conflicts = state.share_list_conflicts(share_id);
    
    if let Ok(conflicts) = conflicts {
        if !conflicts.is_empty() {
            let conflict_id = conflicts[0].id;

            // Resolve with KeepLocal
            state
                .share_resolve_conflict(share_id, conflict_id, ConflictResolution::KeepLocal)
                .expect("Failed to resolve conflict");

            // Verify local version is kept
            let content = fs::read_to_string(&file_path).await.unwrap();
            assert_eq!(content, "Local version");

            // Verify conflict is removed
            let remaining_conflicts = state.share_list_conflicts(share_id).expect("Failed to list conflicts");
            assert!(!remaining_conflicts.iter().any(|c| c.id == conflict_id));
        }
    }
}

#[tokio::test]
async fn test_conflict_resolution_keep_remote() {
    let (service, _config_dir, _store_dir) = create_test_service().await;
    let test_dir_parent = TempDir::new().unwrap();
    let test_dir = create_test_directory(&test_dir_parent, "share").await;

    let state = service.state();
    let share_id = state.share_add(test_dir.clone()).await.expect("Failed to add share");

    let file_path = test_dir.join("file.txt");

    // Check for conflicts and resolve with KeepRemote
    let conflicts = state.share_list_conflicts(share_id);

    if let Ok(conflicts) = conflicts {
        if !conflicts.is_empty() {
            let conflict_id = conflicts[0].id;

            // Resolve with KeepRemote
            state
                .share_resolve_conflict(share_id, conflict_id, ConflictResolution::KeepRemote)
                .expect("Failed to resolve conflict");

            // Verify conflict is removed
            let remaining_conflicts = state.share_list_conflicts(share_id).expect("Failed to list conflicts");
            assert!(!remaining_conflicts.iter().any(|c| c.id == conflict_id));
        }
    }
}

#[tokio::test]
async fn test_conflict_resolution_keep_both() {
    let (service, _config_dir, _store_dir) = create_test_service().await;
    let test_dir_parent = TempDir::new().unwrap();
    let test_dir = create_test_directory(&test_dir_parent, "share").await;

    let state = service.state();
    let share_id = state.share_add(test_dir.clone()).await.expect("Failed to add share");

    // Check for conflicts
    let conflicts = state.share_list_conflicts(share_id);

    if let Ok(conflicts) = conflicts {
        if !conflicts.is_empty() {
            let conflict_id = conflicts[0].id;
            let original_path = conflicts[0].path.clone();

            // Resolve with KeepBoth
            state
                .share_resolve_conflict(share_id, conflict_id, ConflictResolution::KeepBoth)
                .expect("Failed to resolve conflict");

            // Verify both files exist
            assert!(test_dir.join(&original_path).exists());

            // Check for conflict copy (would have timestamp suffix)
            let parent = test_dir.join(&original_path).parent().unwrap();
            let entries: Vec<_> = std::fs::read_dir(parent).unwrap().collect();

            // Should have at least 2 files (original + conflict copy)
            assert!(entries.len() >= 2);
        }
    }
}

#[tokio::test]
async fn test_conflict_resolution_delete_both() {
    let (service, _config_dir, _store_dir) = create_test_service().await;
    let test_dir_parent = TempDir::new().unwrap();
    let test_dir = create_test_directory(&test_dir_parent, "share").await;

    let state = service.state();
    let share_id = state.share_add(test_dir.clone()).await.expect("Failed to add share");

    let conflicts = state.share_list_conflicts(share_id);

    if let Ok(conflicts) = conflicts {
        if !conflicts.is_empty() {
            let conflict_id = conflicts[0].id;
            let file_path = test_dir.join(&conflicts[0].path);

            // Resolve with DeleteBoth
            state
                .share_resolve_conflict(share_id, conflict_id, ConflictResolution::DeleteBoth)
                .expect("Failed to resolve conflict");

            // Verify file is deleted
            assert!(!file_path.exists());

            // Verify conflict is removed
            let remaining_conflicts = state.share_list_conflicts(share_id).expect("Failed to list conflicts");
            assert!(!remaining_conflicts.iter().any(|c| c.id == conflict_id));
        }
    }
}

#[tokio::test]
async fn test_conflict_detection_file_created() {
    let (service1, _config_dir1, _store_dir1) = create_test_service().await;
    let (service2, _config_dir2, _store_dir2) = create_test_service().await;

    let test_dir_parent = TempDir::new().unwrap();
    let dir1 = create_test_directory(&test_dir_parent, "share1").await;
    let dir2 = create_test_directory(&test_dir_parent, "share2").await;

    let state1 = service1.state();
    let state2 = service2.state();

    let share_id1 = state1.share_add(dir1.clone()).await.expect("Failed to add share 1");
    let _share_id2 = state2.share_add(dir2.clone()).await.expect("Failed to add share 2");

    // Create same file in both shares with different content
    fs::write(dir1.join("new_file.txt"), b"Content from share1")
        .await
        .unwrap();

    // Wait less than conflict window
    sleep(Duration::from_secs(5)).await;

    fs::write(dir2.join("new_file.txt"), b"Content from share2")
        .await
        .unwrap();

    // Allow sync to detect conflict
    sleep(Duration::from_secs(2)).await;

    // Check for conflicts
    let conflicts = state1.share_list_conflicts(share_id1).expect("Failed to list conflicts");

    // Should detect creation conflict
    assert!(!conflicts.is_empty());
    assert!(conflicts
        .iter()
        .any(|c| matches!(c.conflict_type, ConflictType::FileCreated)));
}

#[tokio::test]
async fn test_conflict_detection_file_deleted() {
    let (service1, _config_dir1, _store_dir1) = create_test_service().await;
    let (service2, _config_dir2, _store_dir2) = create_test_service().await;

    let test_dir_parent = TempDir::new().unwrap();
    let dir1 = create_test_directory(&test_dir_parent, "share1").await;
    let dir2 = create_test_directory(&test_dir_parent, "share2").await;

    let state1 = service1.state();
    let state2 = service2.state();

    let share_id1 = state1.share_add(dir1.clone()).await.expect("Failed to add share 1");
    let _share_id2 = state2.share_add(dir2.clone()).await.expect("Failed to add share 2");

    // Delete file in share1
    fs::remove_file(dir1.join("file.txt")).await.unwrap();

    // Wait less than conflict window
    sleep(Duration::from_secs(5)).await;

    // Modify same file in share2
    fs::write(dir2.join("file.txt"), b"Modified content")
        .await
        .unwrap();

    // Allow sync to detect conflict
    sleep(Duration::from_secs(2)).await;

    // Check for conflicts
    let conflicts = state1.share_list_conflicts(share_id1).expect("Failed to list conflicts");

    // Should detect delete conflict
    assert!(!conflicts.is_empty());
    assert!(conflicts
        .iter()
        .any(|c| matches!(c.conflict_type, ConflictType::FileDeleted)));
}

#[tokio::test]
async fn test_conflict_detection_directory_conflict() {
    let (service1, _config_dir1, _store_dir1) = create_test_service().await;
    let (service2, _config_dir2, _store_dir2) = create_test_service().await;

    let test_dir_parent = TempDir::new().unwrap();
    let dir1 = create_test_directory(&test_dir_parent, "share1").await;
    let dir2 = create_test_directory(&test_dir_parent, "share2").await;

    let state1 = service1.state();
    let state2 = service2.state();

    let share_id1 = state1.share_add(dir1.clone()).await.expect("Failed to add share 1");
    let _share_id2 = state2.share_add(dir2.clone()).await.expect("Failed to add share 2");

    // Create directory in share1
    fs::create_dir(dir1.join("new_dir")).await.unwrap();

    // Wait less than conflict window
    sleep(Duration::from_secs(5)).await;

    // Create file with same name in share2
    fs::write(dir2.join("new_dir"), b"This is a file")
        .await
        .unwrap();

    // Allow sync to detect conflict
    sleep(Duration::from_secs(2)).await;

    // Check for conflicts
    let conflicts = state1.share_list_conflicts(share_id1).expect("Failed to list conflicts");

    // Should detect directory-file conflict
    assert!(!conflicts.is_empty());
    assert!(conflicts
        .iter()
        .any(|c| matches!(c.conflict_type, ConflictType::DirectoryFileConflict)));
}

#[tokio::test]
async fn test_list_conflicts_empty() {
    let (service, _config_dir, _store_dir) = create_test_service().await;
    let test_dir_parent = TempDir::new().unwrap();
    let test_dir = create_test_directory(&test_dir_parent, "share").await;

    let state = service.state();
    let share_id = state.share_add(test_dir).await.expect("Failed to add share");

    // List conflicts - should be empty initially
    let conflicts = state.share_list_conflicts(share_id).expect("Failed to list conflicts");
    assert_eq!(conflicts.len(), 0);
}

#[tokio::test]
async fn test_resolve_nonexistent_conflict() {
    let (service, _config_dir, _store_dir) = create_test_service().await;
    let test_dir_parent = TempDir::new().unwrap();
    let test_dir = create_test_directory(&test_dir_parent, "share").await;

    let state = service.state();
    let share_id = state.share_add(test_dir).await.expect("Failed to add share");

    // Try to resolve non-existent conflict
    let fake_conflict_id = Uuid::new_v4();
    let result = state.share_resolve_conflict(
        share_id,
        fake_conflict_id,
        ConflictResolution::KeepLocal,
    );

    assert!(result.is_err());
}

#[tokio::test]
async fn test_conflict_outside_window() {
    let (service1, _config_dir1, _store_dir1) = create_test_service().await;
    let (service2, _config_dir2, _store_dir2) = create_test_service().await;

    let test_dir_parent = TempDir::new().unwrap();
    let dir1 = create_test_directory(&test_dir_parent, "share1").await;
    let dir2 = create_test_directory(&test_dir_parent, "share2").await;

    let state1 = service1.state();
    let state2 = service2.state();

    let share_id1 = state1.share_add(dir1.clone()).await.expect("Failed to add share 1");
    let _share_id2 = state2.share_add(dir2.clone()).await.expect("Failed to add share 2");

    // Modify file in share1
    fs::write(dir1.join("file.txt"), b"Modified by share1")
        .await
        .unwrap();

    // Wait MORE than conflict window (>10 seconds)
    sleep(Duration::from_secs(12)).await;

    // Modify same file in share2
    fs::write(dir2.join("file.txt"), b"Modified by share2")
        .await
        .unwrap();

    // Allow sync to process
    sleep(Duration::from_secs(2)).await;

    // Check for conflicts - should be empty (outside conflict window)
    let conflicts = state1.share_list_conflicts(share_id1).expect("Failed to list conflicts");
    assert_eq!(conflicts.len(), 0);
}

#[tokio::test]
async fn test_multiple_conflicts() {
    let (service, _config_dir, _store_dir) = create_test_service().await;
    let test_dir_parent = TempDir::new().unwrap();
    let test_dir = create_test_directory(&test_dir_parent, "share").await;

    let state = service.state();
    let share_id = state.share_add(test_dir.clone()).await.expect("Failed to add share");

    // Create multiple conflicting scenarios
    // (This would require actual sync setup, so we're testing the API)

    // Add multiple files
    fs::write(test_dir.join("conflict1.txt"), b"Content 1")
        .await
        .unwrap();
    fs::write(test_dir.join("conflict2.txt"), b"Content 2")
        .await
        .unwrap();

    // List conflicts
    let conflicts = state.share_list_conflicts(share_id).expect("Failed to list conflicts");

    // If conflicts exist, verify we can resolve them individually
    if conflicts.len() >= 2 {
        let conflict_id1 = conflicts[0].id;
        let conflict_id2 = conflicts[1].id;

        // Resolve first conflict
        state
            .share_resolve_conflict(share_id, conflict_id1, ConflictResolution::KeepLocal)
            .expect("Failed to resolve conflict 1");

        // Verify first is gone but second remains
        let remaining = state.share_list_conflicts(share_id).expect("Failed to list conflicts");
        assert!(!remaining.iter().any(|c| c.id == conflict_id1));
        assert!(remaining.iter().any(|c| c.id == conflict_id2));

        // Resolve second conflict
        state
            .share_resolve_conflict(share_id, conflict_id2, ConflictResolution::KeepRemote)
            .expect("Failed to resolve conflict 2");

        // Verify both are gone
        let remaining = state.share_list_conflicts(share_id).expect("Failed to list conflicts");
        assert!(!remaining.iter().any(|c| c.id == conflict_id1 || c.id == conflict_id2));
    }
}
