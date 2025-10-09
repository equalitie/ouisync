//! Bidirectional sync bridge between local filesystem and Ouisync repository
//!
//! This module provides real-time synchronization in both directions:
//! - Filesystem changes → Repository (via file watcher)
//! - Repository changes → Filesystem (via repository events)

use crate::{conflict, error::Error};
use notify::{
    event::{CreateKind, ModifyKind, RemoveKind},
    Event as NotifyEvent, EventKind, RecommendedWatcher, RecursiveMode, Watcher,
};
use ouisync::{Event as RepoEvent, File, Repository};
use scoped_task::ScopedJoinHandle;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{
    fs,
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
    select,
    sync::mpsc,
    time::sleep,
};
use tracing::{debug, error, warn};

/// Handle to a sync bridge that can be used to control it
pub struct SyncBridge {
    _task: ScopedJoinHandle<()>,
    _watcher: RecommendedWatcher,
}

impl SyncBridge {
    /// Create a new sync bridge for the given directory and repository
    pub async fn new(local_path: PathBuf, repository: Arc<Repository>) -> Result<Self, Error> {
        // Create channel for filesystem events
        let (fs_tx, mut fs_rx) = mpsc::channel(100);

        // Create file watcher
        let mut watcher = notify::recommended_watcher(move |res: Result<NotifyEvent, _>| {
            if let Ok(event) = res {
                let _ = fs_tx.blocking_send(event);
            }
        })
        .map_err(|e| Error::Other(e.to_string()))?;

        // Watch the local directory
        watcher
            .watch(&local_path, RecursiveMode::Recursive)
            .map_err(|e| Error::Other(e.to_string()))?;

        // Subscribe to repository events
        let mut repo_events = repository.subscribe();

        let repo_clone = repository.clone();
        let path_clone = local_path.clone();

        // Spawn sync task
        let task = scoped_task::spawn(async move {
            // Initial sync: copy everything from repo to filesystem
            if let Err(e) = sync_repo_to_fs(&path_clone, &repo_clone).await {
                error!("Initial sync failed: {:?}", e);
            }

            loop {
                select! {
                    // Handle filesystem changes
                    Some(event) = fs_rx.recv() => {
                        if let Err(e) = handle_fs_event(&path_clone, &repo_clone, event).await {
                            error!("Error handling filesystem event: {:?}", e);
                        }
                    }
                    // Handle repository changes
                    Ok(event) = repo_events.recv() => {
                        if let Err(e) = handle_repo_event(&path_clone, &repo_clone, event).await {
                            error!("Error handling repository event: {:?}", e);
                        }
                    }
                    else => break,
                }
            }

            debug!("Sync bridge task stopped");
        });

        Ok(Self {
            _task: task,
            _watcher: watcher,
        })
    }
}

/// Handle filesystem events and sync to repository
async fn handle_fs_event(
    local_path: &Path,
    repo: &Repository,
    event: NotifyEvent,
) -> Result<(), Error> {
    match event.kind {
        EventKind::Create(CreateKind::File) | EventKind::Modify(ModifyKind::Data(_)) => {
            for path in event.paths {
                // Get relative path
                let rel_path = match path.strip_prefix(local_path) {
                    Ok(p) => p,
                    Err(_) => continue,
                };

                // Skip hidden files and repository database
                if is_ignored_path(rel_path) {
                    continue;
                }

                debug!("File created/modified: {:?}", rel_path);

                // Check for conflict before syncing
                let repo_path = rel_path.to_string_lossy().to_string();
                if let Err(e) = check_and_handle_conflict(local_path, repo, &path, &repo_path).await {
                    error!("Failed to handle potential conflict: {:?}", e);
                    continue;
                }

                // Read file content
                let content = match fs::read(&path).await {
                    Ok(c) => c,
                    Err(e) => {
                        warn!("Failed to read file {:?}: {}", path, e);
                        continue;
                    }
                };

                // Write to repository
                match write_to_repo(repo, &repo_path, &content).await {
                    Ok(_) => debug!("Synced to repo: {}", repo_path),
                    Err(e) => error!("Failed to write to repo: {:?}", e),
                }
            }
        }
        EventKind::Remove(RemoveKind::File) => {
            for path in event.paths {
                let rel_path = match path.strip_prefix(local_path) {
                    Ok(p) => p,
                    Err(_) => continue,
                };

                if is_ignored_path(rel_path) {
                    continue;
                }

                debug!("File removed: {:?}", rel_path);

                // Remove from repository
                let repo_path = rel_path.to_string_lossy().to_string();
                if let Err(e) = repo.remove_entry(&repo_path).await {
                    // Ignore "not found" errors
                    if !matches!(e, ouisync::Error::EntryNotFound) {
                        error!("Failed to remove from repo: {:?}", e);
                    }
                }
            }
        }
        EventKind::Create(CreateKind::Folder) => {
            for path in event.paths {
                let rel_path = match path.strip_prefix(local_path) {
                    Ok(p) => p,
                    Err(_) => continue,
                };

                if is_ignored_path(rel_path) {
                    continue;
                }

                debug!("Directory created: {:?}", rel_path);

                // Create directory in repository
                let repo_path = rel_path.to_string_lossy().to_string();
                if let Err(e) = repo.create_directory(&repo_path).await {
                    // Ignore "already exists" errors
                    if !matches!(e, ouisync::Error::EntryExists) {
                        error!("Failed to create directory in repo: {:?}", e);
                    }
                }
            }
        }
        EventKind::Remove(RemoveKind::Folder) => {
            for path in event.paths {
                let rel_path = match path.strip_prefix(local_path) {
                    Ok(p) => p,
                    Err(_) => continue,
                };

                if is_ignored_path(rel_path) {
                    continue;
                }

                debug!("Directory removed: {:?}", rel_path);

                // Remove directory from repository
                let repo_path = rel_path.to_string_lossy().to_string();
                if let Err(e) = repo.remove_entry(&repo_path).await {
                    if !matches!(e, ouisync::Error::EntryNotFound) {
                        error!("Failed to remove directory from repo: {:?}", e);
                    }
                }
            }
        }
        _ => {}
    }

    Ok(())
}

/// Handle repository events and sync to filesystem
async fn handle_repo_event(
    local_path: &Path,
    repo: &Repository,
    event: RepoEvent,
) -> Result<(), Error> {
    // For now, we'll do a simple implementation that refreshes on any change
    // A more sophisticated implementation would track specific paths
    match event.payload {
        ouisync::Payload::BranchChanged(_) => {
            debug!("Repository changed, syncing to filesystem");
            // Small delay to batch changes
            sleep(Duration::from_millis(500)).await;
            sync_repo_to_fs(local_path, repo).await?;
        }
        _ => {}
    }

    Ok(())
}

/// Sync entire repository to filesystem
async fn sync_repo_to_fs(local_path: &Path, repo: &Repository) -> Result<(), Error> {
    // Create local directory if it doesn't exist
    fs::create_dir_all(local_path)
        .await
        .map_err(|e| Error::Other(format!("Failed to create directory: {}", e)))?;

    // Recursively sync
    sync_directory_to_fs(local_path, repo, "").await
}

/// Recursively sync a directory from repository to filesystem
async fn sync_directory_to_fs(
    local_path: &Path,
    repo: &Repository,
    repo_path: &str,
) -> Result<(), Error> {
    let entries = repo
        .list_directory(repo_path)
        .await
        .map_err(|e| Error::Other(format!("Failed to list directory: {}", e)))?;

    for entry in entries {
        let entry_name = entry.name();
        let entry_repo_path = if repo_path.is_empty() {
            entry_name.to_owned()
        } else {
            format!("{}/{}", repo_path, entry_name)
        };
        let entry_fs_path = local_path.join(entry_name);

        match entry.entry_type() {
            ouisync::EntryType::Directory => {
                // Create directory in filesystem
                if !entry_fs_path.exists() {
                    fs::create_dir(&entry_fs_path)
                        .await
                        .map_err(|e| Error::Other(format!("Failed to create directory: {}", e)))?;
                }
                // Recurse
                sync_directory_to_fs(&entry_fs_path, repo, &entry_repo_path).await?;
            }
            ouisync::EntryType::File => {
                // Read from repository
                let content = read_from_repo(repo, &entry_repo_path).await?;

                // Check if file exists and has same content
                let needs_update = match fs::read(&entry_fs_path).await {
                    Ok(existing) => existing != content,
                    Err(_) => true,
                };

                if needs_update {
                    // Write to filesystem
                    fs::write(&entry_fs_path, content)
                        .await
                        .map_err(|e| Error::Other(format!("Failed to write file: {}", e)))?;
                    debug!("Synced from repo to fs: {:?}", entry_fs_path);
                }
            }
        }
    }

    Ok(())
}

/// Write content to repository file
async fn write_to_repo(repo: &Repository, path: &str, content: &[u8]) -> Result<(), Error> {
    let mut file = repo
        .open_file(path)
        .await
        .or_else(|_| async { repo.create_file(path).await })
        .await
        .map_err(|e| Error::Other(format!("Failed to open/create file: {}", e)))?;

    file.truncate(0)
        .await
        .map_err(|e| Error::Other(format!("Failed to truncate file: {}", e)))?;

    file.write_all(content)
        .await
        .map_err(|e| Error::Other(format!("Failed to write file: {}", e)))?;

    file.flush()
        .await
        .map_err(|e| Error::Other(format!("Failed to flush file: {}", e)))?;

    Ok(())
}

/// Read content from repository file
async fn read_from_repo(repo: &Repository, path: &str) -> Result<Vec<u8>, Error> {
    let mut file = repo
        .open_file(path)
        .await
        .map_err(|e| Error::Other(format!("Failed to open file: {}", e)))?;

    let mut content = Vec::new();
    file.read_to_end(&mut content)
        .await
        .map_err(|e| Error::Other(format!("Failed to read file: {}", e)))?;

    Ok(content)
}

/// Check if a path should be ignored (hidden files, .ouisyncdb, etc.)
fn is_ignored_path(path: &Path) -> bool {
    path.components().any(|c| {
        let name = c.as_os_str().to_string_lossy();
        name.starts_with('.') || name.ends_with(".ouisyncdb")
    })
}

/// Check for conflicts and handle them
///
/// A conflict occurs when:
/// - The file exists in both the repository and filesystem
/// - The repository version has been modified more recently than the filesystem version
/// - The filesystem version is being modified locally
///
/// When a conflict is detected, the local version is renamed with a .sync-conflict suffix
async fn check_and_handle_conflict(
    local_root: &Path,
    repo: &Repository,
    fs_path: &Path,
    repo_path: &str,
) -> Result<(), Error> {
    // Skip conflict files themselves
    if conflict::is_conflict_file(fs_path) {
        return Ok(());
    }

    // Check if file exists in repository
    let repo_file_exists = match repo.lookup_type(repo_path).await {
        Ok(ouisync::EntryType::File) => true,
        Ok(_) => false,
        Err(ouisync::Error::EntryNotFound) => false,
        Err(e) => {
            warn!("Failed to lookup file in repo: {:?}", e);
            return Ok(()); // Don't fail sync on lookup errors
        }
    };

    if !repo_file_exists {
        // New file, no conflict possible
        return Ok(());
    }

    // Get filesystem modified time
    let fs_metadata = match tokio::fs::metadata(fs_path).await {
        Ok(m) => m,
        Err(e) => {
            warn!("Failed to get filesystem metadata: {:?}", e);
            return Ok(());
        }
    };

    let fs_modified = match fs_metadata.modified() {
        Ok(t) => t,
        Err(e) => {
            warn!("Failed to get filesystem modified time: {:?}", e);
            return Ok(());
        }
    };

    // Get repository content to compare
    let repo_content = match read_from_repo(repo, repo_path).await {
        Ok(c) => c,
        Err(e) => {
            warn!("Failed to read from repo: {:?}", e);
            return Ok(());
        }
    };

    // Read local content
    let fs_content = match tokio::fs::read(fs_path).await {
        Ok(c) => c,
        Err(e) => {
            warn!("Failed to read local file: {:?}", e);
            return Ok(());
        }
    };

    // If contents are the same, no conflict
    if repo_content == fs_content {
        return Ok(());
    }

    // Contents are different - check if there was a recent change from network
    // We use a heuristic: if the file was recently synced from the network,
    // and now the local version differs, it's likely a conflict
    
    // For simplicity, we'll consider it a conflict if:
    // 1. The repository has content (was synced)
    // 2. The local content differs
    // 3. The modification is happening "now" (within last few seconds)
    
    let now = std::time::SystemTime::now();
    let time_since_mod = now.duration_since(fs_modified).unwrap_or(Duration::from_secs(0));
    
    // If the file was modified very recently (< 10 seconds ago), it might be
    // a conflict with a network sync
    if time_since_mod < Duration::from_secs(10) {
        // CONFLICT DETECTED
        warn!("Conflict detected for {:?}", fs_path);
        
        // Rename local file to conflict version
        let conflict_path = conflict::generate_conflict_filename(fs_path);
        
        debug!("Renaming {:?} to {:?}", fs_path, conflict_path);
        
        if let Err(e) = tokio::fs::rename(fs_path, &conflict_path).await {
            error!("Failed to rename conflicted file: {:?}", e);
            return Err(Error::Other(format!("Failed to rename conflicted file: {}", e)));
        }
        
        // Write the repository version to the original path
        if let Err(e) = tokio::fs::write(fs_path, &repo_content).await {
            error!("Failed to write repository version: {:?}", e);
            // Try to restore the original
            let _ = tokio::fs::rename(&conflict_path, fs_path).await;
            return Err(Error::Other(format!("Failed to write repository version: {}", e)));
        }
        
        debug!("Conflict resolved: kept both versions");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ouisync::{AccessSecrets, Credentials, RepositoryParams};
    use tempfile::TempDir;

    /// Helper to create a test repository
    async fn create_test_repo() -> (Repository, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let repo_path = temp_dir.path().join("test.db");
        let params = RepositoryParams::new(repo_path);
        let secrets = AccessSecrets::random_write();
        let credentials = Credentials::with_random_password(secrets);
        let repo = Repository::create(&params, credentials).await.unwrap();
        (repo, temp_dir)
    }

    #[test]
    fn test_is_ignored_path() {
        assert!(is_ignored_path(Path::new(".hidden")));
        assert!(is_ignored_path(Path::new(".git/config")));
        assert!(is_ignored_path(Path::new("dir/.hidden")));
        assert!(is_ignored_path(Path::new(".ouisyncdb")));
        assert!(is_ignored_path(Path::new("dir/.ouisyncdb")));
        
        assert!(!is_ignored_path(Path::new("normal.txt")));
        assert!(!is_ignored_path(Path::new("dir/file.txt")));
    }

    #[tokio::test]
    async fn test_write_and_read_repo() {
        let (repo, _temp_dir) = create_test_repo().await;
        let content = b"Hello, World!";
        let path = "test.txt";

        // Write
        write_to_repo(&repo, path, content).await.unwrap();

        // Read back
        let read_content = read_from_repo(&repo, path).await.unwrap();
        assert_eq!(read_content, content);
    }

    #[tokio::test]
    async fn test_write_overwrites_existing() {
        let (repo, _temp_dir) = create_test_repo().await;
        let path = "test.txt";

        // Write first time
        write_to_repo(&repo, path, b"First").await.unwrap();
        
        // Write second time (overwrite)
        write_to_repo(&repo, path, b"Second").await.unwrap();

        // Read back - should be second content
        let content = read_from_repo(&repo, path).await.unwrap();
        assert_eq!(content, b"Second");
    }

    #[tokio::test]
    async fn test_sync_repo_to_fs_empty_repo() {
        let (repo, _repo_temp) = create_test_repo().await;
        let fs_temp = TempDir::new().unwrap();
        let local_path = fs_temp.path();

        // Sync empty repo to filesystem
        sync_repo_to_fs(local_path, &repo).await.unwrap();

        // Directory should exist but be empty (except for hidden files)
        assert!(local_path.exists());
        let entries: Vec<_> = std::fs::read_dir(local_path).unwrap().collect();
        assert_eq!(entries.len(), 0);
    }

    #[tokio::test]
    async fn test_sync_repo_to_fs_with_files() {
        let (repo, _repo_temp) = create_test_repo().await;
        let fs_temp = TempDir::new().unwrap();
        let local_path = fs_temp.path();

        // Add files to repo
        write_to_repo(&repo, "file1.txt", b"Content 1").await.unwrap();
        write_to_repo(&repo, "file2.txt", b"Content 2").await.unwrap();

        // Create a subdirectory with a file
        repo.create_directory("subdir").await.unwrap();
        write_to_repo(&repo, "subdir/file3.txt", b"Content 3").await.unwrap();

        // Sync to filesystem
        sync_repo_to_fs(local_path, &repo).await.unwrap();

        // Verify files exist
        assert!(local_path.join("file1.txt").exists());
        assert!(local_path.join("file2.txt").exists());
        assert!(local_path.join("subdir").is_dir());
        assert!(local_path.join("subdir/file3.txt").exists());

        // Verify content
        let content1 = std::fs::read(local_path.join("file1.txt")).unwrap();
        assert_eq!(content1, b"Content 1");
        
        let content3 = std::fs::read(local_path.join("subdir/file3.txt")).unwrap();
        assert_eq!(content3, b"Content 3");
    }

    #[tokio::test]
    async fn test_sync_bridge_creation() {
        let (repo, _repo_temp) = create_test_repo().await;
        let fs_temp = TempDir::new().unwrap();
        let local_path = fs_temp.path().to_owned();

        // Create sync bridge
        let bridge = SyncBridge::new(local_path.clone(), Arc::new(repo)).await;
        assert!(bridge.is_ok());
    }

    #[tokio::test]
    async fn test_conflict_detection_window() {
        // This test verifies the 10-second conflict window logic
        let (repo, _repo_temp) = create_test_repo().await;
        let fs_temp = TempDir::new().unwrap();
        let local_path = fs_temp.path();
        let file_path = local_path.join("test.txt");

        // Create file in repo
        write_to_repo(&repo, "test.txt", b"Repo version").await.unwrap();

        // Create file in filesystem with different content
        tokio::fs::write(&file_path, b"Local version").await.unwrap();

        // Check for conflict (should detect within 10-second window)
        let result = check_and_handle_conflict(
            local_path,
            &repo,
            &file_path,
            "test.txt"
        ).await;

        // Should succeed without error
        assert!(result.is_ok());

        // Conflict file should be created
        let conflict_files: Vec<_> = std::fs::read_dir(local_path)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .contains(".sync-conflict-")
            })
            .collect();

        assert_eq!(conflict_files.len(), 1, "Should create one conflict file");
    }
}
