//! Integration tests for Share Management
//!
//! These tests verify the complete share lifecycle including:
//! - Adding and removing shares
//! - Peer management
//! - Configuration persistence
//! - Share tokens

use ouisync::{AccessMode, PeerAddr};
use ouisync_service::{Error, Service};
use std::path::PathBuf;
use tempfile::TempDir;
use tokio::fs;
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
async fn create_test_directory(parent: &TempDir) -> PathBuf {
    let test_dir = parent.path().join("test_share");
    fs::create_dir_all(&test_dir)
        .await
        .expect("Failed to create test directory");
    
    // Create some test files
    fs::write(test_dir.join("file1.txt"), b"Hello, World!")
        .await
        .expect("Failed to create file1");
    fs::write(test_dir.join("file2.txt"), b"Test content")
        .await
        .expect("Failed to create file2");
    
    // Create subdirectory with file
    let subdir = test_dir.join("subdir");
    fs::create_dir(&subdir).await.expect("Failed to create subdir");
    fs::write(subdir.join("file3.txt"), b"Nested file")
        .await
        .expect("Failed to create file3");
    
    test_dir
}

#[tokio::test]
async fn test_add_share() {
    let (service, _config_dir, _store_dir) = create_test_service().await;
    let test_dir_parent = TempDir::new().unwrap();
    let test_dir = create_test_directory(&test_dir_parent).await;
    
    let state = service.state();
    
    // Add share
    let share_id = state
        .share_add(test_dir.clone())
        .await
        .expect("Failed to add share");
    
    // Verify it's a valid UUID
    assert_ne!(share_id, Uuid::nil());
    
    // Verify .ouisyncdb was created
    assert!(test_dir.join(".ouisyncdb").exists());
    
    // Get share info
    let info = state
        .share_get_info(share_id)
        .expect("Failed to get share info");
    
    assert_eq!(info.id, share_id);
    assert_eq!(info.path, test_dir);
    assert_eq!(info.name, "test_share");
    assert_eq!(info.peers.len(), 0);
    assert!(info.sync_enabled);
}

#[tokio::test]
async fn test_add_duplicate_share() {
    let (service, _config_dir, _store_dir) = create_test_service().await;
    let test_dir_parent = TempDir::new().unwrap();
    let test_dir = create_test_directory(&test_dir_parent).await;
    
    let state = service.state();
    
    // Add share first time
    let _share_id = state
        .share_add(test_dir.clone())
        .await
        .expect("Failed to add share");
    
    // Try to add same directory again - should fail
    let result = state.share_add(test_dir.clone()).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_list_shares() {
    let (service, _config_dir, _store_dir) = create_test_service().await;
    let test_dir_parent = TempDir::new().unwrap();
    
    let state = service.state();
    
    // Initially empty
    let shares = state.share_list();
    assert_eq!(shares.len(), 0);
    
    // Add first share
    let dir1 = create_test_directory(&test_dir_parent).await;
    let share_id1 = state.share_add(dir1).await.expect("Failed to add share 1");
    
    // Add second share
    let dir2 = test_dir_parent.path().join("test_share_2");
    fs::create_dir_all(&dir2).await.unwrap();
    let share_id2 = state.share_add(dir2).await.expect("Failed to add share 2");
    
    // List should have 2 shares
    let shares = state.share_list();
    assert_eq!(shares.len(), 2);
    
    let ids: Vec<Uuid> = shares.iter().map(|s| s.id).collect();
    assert!(ids.contains(&share_id1));
    assert!(ids.contains(&share_id2));
}

#[tokio::test]
async fn test_remove_share() {
    let (service, _config_dir, _store_dir) = create_test_service().await;
    let test_dir_parent = TempDir::new().unwrap();
    let test_dir = create_test_directory(&test_dir_parent).await;
    
    let state = service.state();
    
    // Add share
    let share_id = state.share_add(test_dir.clone()).await.expect("Failed to add share");
    
    // Verify it exists
    assert_eq!(state.share_list().len(), 1);
    
    // Remove it (without deleting repo file)
    state
        .share_remove(share_id, false)
        .await
        .expect("Failed to remove share");
    
    // Verify it's gone
    assert_eq!(state.share_list().len(), 0);
    
    // Verify .ouisyncdb still exists
    assert!(test_dir.join(".ouisyncdb").exists());
}

#[tokio::test]
async fn test_remove_share_with_delete() {
    let (service, _config_dir, _store_dir) = create_test_service().await;
    let test_dir_parent = TempDir::new().unwrap();
    let test_dir = create_test_directory(&test_dir_parent).await;
    
    let state = service.state();
    
    // Add share
    let share_id = state.share_add(test_dir.clone()).await.expect("Failed to add share");
    
    // Remove it WITH deleting repo file
    state
        .share_remove(share_id, true)
        .await
        .expect("Failed to remove share");
    
    // Verify .ouisyncdb is deleted
    assert!(!test_dir.join(".ouisyncdb").exists());
}

#[tokio::test]
async fn test_peer_management() {
    let (service, _config_dir, _store_dir) = create_test_service().await;
    let test_dir_parent = TempDir::new().unwrap();
    let test_dir = create_test_directory(&test_dir_parent).await;
    
    let state = service.state();
    
    // Add share
    let share_id = state.share_add(test_dir).await.expect("Failed to add share");
    
    // Initially no peers
    let peers = state.share_list_peers(share_id).expect("Failed to list peers");
    assert_eq!(peers.len(), 0);
    
    // Add a peer
    let peer1: PeerAddr = "quic/127.0.0.1:20209".parse().unwrap();
    state
        .share_add_peer(share_id, peer1)
        .expect("Failed to add peer");
    
    // Verify peer was added
    let peers = state.share_list_peers(share_id).expect("Failed to list peers");
    assert_eq!(peers.len(), 1);
    assert_eq!(peers[0], peer1);
    
    // Add another peer
    let peer2: PeerAddr = "tcp/192.168.1.100:20210".parse().unwrap();
    state
        .share_add_peer(share_id, peer2)
        .expect("Failed to add peer");
    
    let peers = state.share_list_peers(share_id).expect("Failed to list peers");
    assert_eq!(peers.len(), 2);
    
    // Remove first peer
    state
        .share_remove_peer(share_id, peer1)
        .expect("Failed to remove peer");
    
    let peers = state.share_list_peers(share_id).expect("Failed to list peers");
    assert_eq!(peers.len(), 1);
    assert_eq!(peers[0], peer2);
}

#[tokio::test]
async fn test_sync_enabled() {
    let (service, _config_dir, _store_dir) = create_test_service().await;
    let test_dir_parent = TempDir::new().unwrap();
    let test_dir = create_test_directory(&test_dir_parent).await;
    
    let state = service.state();
    
    // Add share (sync enabled by default)
    let share_id = state.share_add(test_dir).await.expect("Failed to add share");
    
    let info = state.share_get_info(share_id).expect("Failed to get info");
    assert!(info.sync_enabled);
    
    // Disable sync
    state
        .share_set_sync_enabled(share_id, false)
        .expect("Failed to disable sync");
    
    let info = state.share_get_info(share_id).expect("Failed to get info");
    assert!(!info.sync_enabled);
    
    // Re-enable sync
    state
        .share_set_sync_enabled(share_id, true)
        .expect("Failed to enable sync");
    
    let info = state.share_get_info(share_id).expect("Failed to get info");
    assert!(info.sync_enabled);
}

#[tokio::test]
async fn test_share_token() {
    let (service, _config_dir, _store_dir) = create_test_service().await;
    let test_dir_parent = TempDir::new().unwrap();
    let test_dir = create_test_directory(&test_dir_parent).await;
    
    let state = service.state();
    
    // Add share
    let share_id = state.share_add(test_dir).await.expect("Failed to add share");
    
    // Get write token
    let token = state
        .share_get_sync_token(share_id, AccessMode::Write)
        .expect("Failed to get sync token");
    
    // Token should be non-empty string
    assert!(!token.is_empty());
    
    // Can also get read token
    let read_token = state
        .share_get_sync_token(share_id, AccessMode::Read)
        .expect("Failed to get read token");
    
    assert!(!read_token.is_empty());
    assert_ne!(token, read_token); // Should be different
}

#[tokio::test]
async fn test_config_persistence() {
    let config_dir = TempDir::new().unwrap();
    let store_dir = TempDir::new().unwrap();
    let test_dir_parent = TempDir::new().unwrap();
    let test_dir = create_test_directory(&test_dir_parent).await;
    
    let share_id;
    let peer: PeerAddr = "quic/127.0.0.1:20209".parse().unwrap();
    
    // Create service and add share
    {
        let mut service = Service::init(config_dir.path().to_owned())
            .await
            .expect("Failed to initialize service");
        
        service.set_store_dir(store_dir.path()).await.expect("Failed to set store dir");
        
        let state = service.state();
        
        share_id = state
            .share_add(test_dir.clone())
            .await
            .expect("Failed to add share");
        
        state
            .share_add_peer(share_id, peer)
            .expect("Failed to add peer");
        
        // Verify share exists
        assert_eq!(state.share_list().len(), 1);
    }
    
    // Create new service with same config directory
    {
        let mut service = Service::init(config_dir.path().to_owned())
            .await
            .expect("Failed to initialize service");
        
        service.set_store_dir(store_dir.path()).await.expect("Failed to set store dir");
        
        let state = service.state();
        
        // Share should be reloaded
        let shares = state.share_list();
        assert_eq!(shares.len(), 1);
        
        let info = &shares[0];
        assert_eq!(info.id, share_id);
        assert_eq!(info.path, test_dir);
        assert_eq!(info.peers.len(), 1);
        assert_eq!(info.peers[0], peer);
    }
}

#[tokio::test]
async fn test_nonexistent_directory() {
    let (service, _config_dir, _store_dir) = create_test_service().await;
    let state = service.state();
    
    // Try to add non-existent directory
    let nonexistent = PathBuf::from("/tmp/this-definitely-does-not-exist-12345");
    let result = state.share_add(nonexistent).await;
    
    assert!(result.is_err());
}

#[tokio::test]
async fn test_relative_path() {
    let (service, _config_dir, _store_dir) = create_test_service().await;
    let state = service.state();
    
    // Try to add relative path - should fail
    let relative = PathBuf::from("relative/path");
    let result = state.share_add(relative).await;
    
    assert!(result.is_err());
}

#[tokio::test]
async fn test_remove_nonexistent_share() {
    let (service, _config_dir, _store_dir) = create_test_service().await;
    let state = service.state();
    
    // Try to remove non-existent share
    let fake_id = Uuid::new_v4();
    let result = state.share_remove(fake_id, false).await;
    
    assert!(result.is_err());
}

#[tokio::test]
async fn test_add_duplicate_peer() {
    let (service, _config_dir, _store_dir) = create_test_service().await;
    let test_dir_parent = TempDir::new().unwrap();
    let test_dir = create_test_directory(&test_dir_parent).await;
    
    let state = service.state();
    let share_id = state.share_add(test_dir).await.expect("Failed to add share");
    
    let peer: PeerAddr = "quic/127.0.0.1:20209".parse().unwrap();
    
    // Add peer first time
    state
        .share_add_peer(share_id, peer)
        .expect("Failed to add peer");
    
    // Try to add same peer again - should fail
    let result = state.share_add_peer(share_id, peer);
    assert!(result.is_err());
}
