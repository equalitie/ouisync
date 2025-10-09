//! Share Management API
//!
//! This module contains the public API functions for managing shares.
//! These functions are called by the language bindings and frontends.

use crate::{
    conflict::{self, ConflictInfo, ConflictResolution},
    error::Error,
    share::{ShareHandle, ShareHolder, ShareInfo, ShareSet},
    sync_bridge::SyncBridge,
};
use ouisync::{
    Access, AccessMode, AccessSecrets, Credentials, Network, PeerAddr, Repository, RepositoryParams,
};
use ouisync_macros::api;
use state_monitor::StateMonitor;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use uuid::Uuid;

impl crate::state::State {
    /// Adds a new local directory as a share for synchronization.
    ///
    /// This function:
    /// 1. Creates a new Ouisync repository (`.ouisyncdb` file in or near the directory)
    /// 2. Initializes bidirectional sync between the directory and repository
    /// 3. Returns the UUID of the newly created share
    ///
    /// # Arguments
    /// * `path` - Absolute path to the local directory to be synchronized
    ///
    /// # Returns
    /// * `Ok(Uuid)` - The unique ID of the created share
    /// * `Err(Error)` - If the directory doesn't exist, is already a share, or creation fails
    #[api]
    pub async fn share_add(&self, path: PathBuf) -> Result<Uuid, Error> {
        // Validate that the path is absolute and exists
        if !path.is_absolute() {
            return Err(Error::Other("Path must be absolute".to_string()));
        }

        if !tokio::fs::try_exists(&path).await.unwrap_or(false) {
            return Err(Error::Other(format!("Directory does not exist: {:?}", path)));
        }

        // Check if this path is already a share
        if self.shares.find_by_path(&path).is_some() {
            return Err(Error::Other(format!(
                "Directory is already a share: {:?}",
                path
            )));
        }

        // Generate unique ID for this share
        let share_id = Uuid::new_v4();

        // Determine repository database path (inside the directory as .ouisyncdb)
        let repo_path = path.join(".ouisyncdb");

        // Create name from directory name
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unnamed")
            .to_string();

        // Create repository with write access
        let params = RepositoryParams::new(repo_path.clone());

        // Generate new credentials for this repository
        let access_secrets = AccessSecrets::random_write();
        let credentials = Credentials::with_random_password(access_secrets);

        let repository = Repository::create(&params, credentials)
            .await
            .map_err(|e| Error::Other(format!("Failed to create repository: {}", e)))?;

        let repository = Arc::new(repository);

        // Register repository with the network for syncing
        let registration = self
            .network
            .register(repository.handle(), self.repos_monitor.make_child(&name));

        // Enable sync
        registration.set_sync_enabled(true);

        // Create sync bridge for bidirectional sync
        let sync_bridge = SyncBridge::new(path.clone(), repository.clone())
            .await
            .map_err(|e| Error::Other(format!("Failed to create sync bridge: {}", e)))?;

        // Create ShareInfo
        let share_info = ShareInfo {
            id: share_id,
            path: path.clone(),
            name,
            peers: Vec::new(),
            sync_enabled: true,
            repository_id: *repository.secrets().id(),
        };

        // Create ShareHolder
        let holder = ShareHolder::new(share_info, repository, Some(sync_bridge));

        // Insert into ShareSet
        let _handle = self
            .shares
            .try_insert(holder)
            .map_err(|e| Error::Other(format!("Failed to insert share: {}", e)))?;

        // Persist share configuration
        self.save_shares().await?;

        Ok(share_id)
    }

    /// Removes a share from synchronization.
    ///
    /// This function:
    /// 1. Stops the sync bridge
    /// 2. Unregisters the repository from the network
    /// 3. Removes the share from the active set
    /// 4. Optionally deletes the `.ouisyncdb` file
    ///
    /// Note: The actual files in the directory are NOT deleted, only the sync metadata.
    ///
    /// # Arguments
    /// * `share_id` - UUID of the share to remove
    /// * `delete_repo_file` - If true, also delete the `.ouisyncdb` file
    #[api]
    pub async fn share_remove(&self, share_id: Uuid, delete_repo_file: bool) -> Result<(), Error> {
        // Find share handle by ID
        let handle = self
            .shares
            .find_by_id(share_id)
            .ok_or_else(|| Error::Other(format!("Share not found: {}", share_id)))?;

        // Remove from ShareSet (this will drop the sync bridge)
        let holder = self
            .shares
            .remove(handle)
            .ok_or_else(|| Error::Other(format!("Share not found: {}", share_id)))?;

        // Optionally delete the repository database file
        if delete_repo_file {
            let repo_path = holder.path().join(".ouisyncdb");
            if let Err(e) = tokio::fs::remove_file(&repo_path).await {
                tracing::warn!("Failed to delete repository file {:?}: {}", repo_path, e);
            }
        }

        // Remove from persistent configuration
        self.save_shares().await?;

        Ok(())
    }

    /// Lists all currently configured shares.
    ///
    /// Returns a vector of `ShareInfo` containing details about each share.
    #[api]
    pub fn share_list(&self) -> Vec<ShareInfo> {
        self.shares.list()
    }

    /// Gets detailed status for a single share.
    ///
    /// # Arguments
    /// * `share_id` - UUID of the share to query
    ///
    /// # Returns
    /// * `Ok(ShareInfo)` - Detailed information about the share
    /// * `Err(Error)` - If the share is not found
    #[api]
    pub fn share_get_info(&self, share_id: Uuid) -> Result<ShareInfo, Error> {
        let handle = self
            .shares
            .find_by_id(share_id)
            .ok_or_else(|| Error::Other(format!("Share not found: {}", share_id)))?;

        self.shares
            .get(handle)
            .ok_or_else(|| Error::Other(format!("Share not found: {}", share_id)))
    }

    /// Associates a peer address with a specific share.
    ///
    /// This adds the peer to the manual peer list for this share.
    /// The system will attempt to connect to this peer for syncing this share's data.
    ///
    /// # Arguments
    /// * `share_id` - UUID of the share
    /// * `peer_addr` - Peer address in format "PROTO/IP:PORT" (e.g., "quic/192.168.1.100:20209")
    #[api]
    pub fn share_add_peer(&self, share_id: Uuid, peer_addr: PeerAddr) -> Result<(), Error> {
        let handle = self
            .shares
            .find_by_id(share_id)
            .ok_or_else(|| Error::Other(format!("Share not found: {}", share_id)))?;

        let added = self
            .shares
            .with_mut(handle, |holder| {
                let was_added = holder.add_peer(peer_addr);
                if was_added {
                    // Add peer to network for this repository
                    self.network.add_user_provided_peer(&peer_addr);
                }
                was_added
            })
            .ok_or_else(|| Error::Other(format!("Share not found: {}", share_id)))?;

        if !added {
            return Err(Error::Other("Peer already exists in share".to_string()));
        }

        // Persist share configuration
        self.save_shares().await?;

        Ok(())
    }

    /// Disassociates a peer address from a specific share.
    ///
    /// # Arguments
    /// * `share_id` - UUID of the share
    /// * `peer_addr` - Peer address to remove
    #[api]
    pub fn share_remove_peer(&self, share_id: Uuid, peer_addr: PeerAddr) -> Result<(), Error> {
        let handle = self
            .shares
            .find_by_id(share_id)
            .ok_or_else(|| Error::Other(format!("Share not found: {}", share_id)))?;

        let removed = self
            .shares
            .with_mut(handle, |holder| {
                let was_removed = holder.remove_peer(&peer_addr);
                if was_removed {
                    // Remove peer from network
                    self.network.remove_user_provided_peer(&peer_addr);
                }
                was_removed
            })
            .ok_or_else(|| Error::Other(format!("Share not found: {}", share_id)))?;

        if !removed {
            return Err(Error::Other("Peer not found in share".to_string()));
        }

        // Persist share configuration
        self.save_shares().await?;

        Ok(())
    }

    /// Lists all peers associated with a specific share.
    ///
    /// # Arguments
    /// * `share_id` - UUID of the share
    ///
    /// # Returns
    /// * `Ok(Vec<PeerAddr>)` - List of peer addresses
    /// * `Err(Error)` - If the share is not found
    #[api]
    pub fn share_list_peers(&self, share_id: Uuid) -> Result<Vec<PeerAddr>, Error> {
        let handle = self
            .shares
            .find_by_id(share_id)
            .ok_or_else(|| Error::Other(format!("Share not found: {}", share_id)))?;

        self.shares
            .with(handle, |holder| holder.peers().to_vec())
            .ok_or_else(|| Error::Other(format!("Share not found: {}", share_id)))
    }

    /// Enables or disables syncing for a specific share.
    ///
    /// # Arguments
    /// * `share_id` - UUID of the share
    /// * `enabled` - Whether to enable or disable syncing
    #[api]
    pub fn share_set_sync_enabled(&self, share_id: Uuid, enabled: bool) -> Result<(), Error> {
        let handle = self
            .shares
            .find_by_id(share_id)
            .ok_or_else(|| Error::Other(format!("Share not found: {}", share_id)))?;

        self.shares
            .with_mut(handle, |holder| {
                holder.set_sync_enabled(enabled);
                // TODO: Actually enable/disable the sync bridge and network registration
            })
            .ok_or_else(|| Error::Other(format!("Share not found: {}", share_id)))?;

        // Persist share configuration
        self.save_shares().await?;

        Ok(())
    }

    /// Gets the sync token for a share that can be used by other peers to sync with this share.
    ///
    /// # Arguments
    /// * `share_id` - UUID of the share
    /// * `access_mode` - Access mode to grant (Read, Write, or Blind)
    ///
    /// # Returns
    /// * `Ok(String)` - The share token as a string
    /// * `Err(Error)` - If the share is not found or token generation fails
    #[api]
    pub fn share_get_sync_token(
        &self,
        share_id: Uuid,
        access_mode: AccessMode,
    ) -> Result<String, Error> {
        let handle = self
            .shares
            .find_by_id(share_id)
            .ok_or_else(|| Error::Other(format!("Share not found: {}", share_id)))?;

        let token = self
            .shares
            .with(handle, |holder| {
                let repo = holder.repository();
                let secrets = repo.secrets();

                // Create share token based on access mode
                let access = match access_mode {
                    AccessMode::Blind => secrets.with_mode(AccessMode::Blind),
                    AccessMode::Read => secrets
                        .with_mode(AccessMode::Read)
                        .map_err(|_| Error::PermissionDenied)?,
                    AccessMode::Write => secrets
                        .with_mode(AccessMode::Write)
                        .map_err(|_| Error::PermissionDenied)?,
                };

                // Generate share token
                let name = holder.name();
                Ok::<_, Error>(access.create_share_token(name).to_string())
            })
            .ok_or_else(|| Error::Other(format!("Share not found: {}", share_id)))??;

        Ok(token)
    }

    /// Lists all conflicts in a share.
    ///
    /// Scans the share directory for `.sync-conflict-*` files and returns information about each.
    ///
    /// # Arguments
    /// * `share_id` - UUID of the share
    ///
    /// # Returns
    /// * `Ok(Vec<ConflictInfo>)` - List of conflicts
    /// * `Err(Error)` - If the share is not found or scan fails
    #[api]
    pub async fn share_list_conflicts(&self, share_id: Uuid) -> Result<Vec<ConflictInfo>, Error> {
        let handle = self
            .shares
            .find_by_id(share_id)
            .ok_or_else(|| Error::Other(format!("Share not found: {}", share_id)))?;

        let share_path = self
            .shares
            .with(handle, |holder| holder.path().to_owned())
            .ok_or_else(|| Error::Other(format!("Share not found: {}", share_id)))?;

        let mut conflicts = Vec::new();

        // Recursively scan for conflict files
        scan_for_conflicts(&share_path, &share_path, &mut conflicts).await?;

        Ok(conflicts)
    }

    /// Resolves a conflict using the specified strategy.
    ///
    /// # Arguments
    /// * `share_id` - UUID of the share
    /// * `conflict_id` - UUID of the conflict to resolve
    /// * `resolution` - How to resolve the conflict
    ///
    /// # Returns
    /// * `Ok(())` - If the conflict was resolved successfully
    /// * `Err(Error)` - If resolution fails
    #[api]
    pub async fn share_resolve_conflict(
        &self,
        share_id: Uuid,
        conflict_id: Uuid,
        resolution: ConflictResolution,
    ) -> Result<(), Error> {
        let handle = self
            .shares
            .find_by_id(share_id)
            .ok_or_else(|| Error::Other(format!("Share not found: {}", share_id)))?;

        let share_path = self
            .shares
            .with(handle, |holder| holder.path().to_owned())
            .ok_or_else(|| Error::Other(format!("Share not found: {}", share_id)))?;

        // Find the conflict
        let mut conflicts = Vec::new();
        scan_for_conflicts(&share_path, &share_path, &mut conflicts).await?;

        let conflict = conflicts
            .into_iter()
            .find(|c| c.id == conflict_id)
            .ok_or_else(|| Error::Other(format!("Conflict not found: {}", conflict_id)))?;

        // Apply resolution strategy
        match resolution {
            ConflictResolution::KeepLocal => {
                // Delete the conflict file, keep the original
                tokio::fs::remove_file(&conflict.conflict_path)
                    .await
                    .map_err(|e| Error::Other(format!("Failed to remove conflict file: {}", e)))?
            }
            ConflictResolution::KeepRemote => {
                // Replace original with conflict, then delete conflict
                tokio::fs::rename(&conflict.conflict_path, &conflict.original_path)
                    .await
                    .map_err(|e| Error::Other(format!("Failed to replace with conflict: {}", e)))?
            }
            ConflictResolution::KeepBoth => {
                // Do nothing, both versions exist
            }
            ConflictResolution::DeleteBoth => {
                // Delete both files
                tokio::fs::remove_file(&conflict.original_path)
                    .await
                    .ok(); // Ignore error if already deleted
                tokio::fs::remove_file(&conflict.conflict_path)
                    .await
                    .map_err(|e| Error::Other(format!("Failed to delete conflict file: {}", e)))?
            }
        }

        Ok(())
    }
}

/// Recursively scan a directory for conflict files
async fn scan_for_conflicts(
    root: &std::path::Path,
    current: &std::path::Path,
    conflicts: &mut Vec<ConflictInfo>,
) -> Result<(), Error> {
    let mut entries = tokio::fs::read_dir(current)
        .await
        .map_err(|e| Error::Other(format!("Failed to read directory: {}", e)))?;

    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| Error::Other(format!("Failed to read entry: {}", e)))?
    {
        let path = entry.path();
        let metadata = entry
            .metadata()
            .await
            .map_err(|e| Error::Other(format!("Failed to get metadata: {}", e)))?;

        if metadata.is_dir() {
            // Recurse into subdirectories
            Box::pin(scan_for_conflicts(root, &path, conflicts)).await?;
        } else if conflict::is_conflict_file(&path) {
            // Found a conflict file
            if let Some(original_path) = conflict::get_original_from_conflict(&path) {
                let conflict_metadata = metadata;
                let conflict_size = conflict_metadata.len();
                let conflict_modified = conflict_metadata
                    .modified()
                    .unwrap_or(std::time::SystemTime::now());

                // Get original file info (if it exists)
                let (original_size, original_modified) =
                    if let Ok(original_metadata) = tokio::fs::metadata(&original_path).await {
                        (
                            original_metadata.len(),
                            original_metadata
                                .modified()
                                .unwrap_or(std::time::SystemTime::now()),
                        )
                    } else {
                        (0, std::time::SystemTime::now())
                    };

                // Make paths relative to root
                let rel_original = original_path
                    .strip_prefix(root)
                    .unwrap_or(&original_path)
                    .to_owned();
                let rel_conflict = path.strip_prefix(root).unwrap_or(&path).to_owned();

                conflicts.push(ConflictInfo {
                    id: Uuid::new_v4(),
                    original_path: rel_original,
                    conflict_path: rel_conflict,
                    detected_at: conflict_modified, // Use conflict file time as detection time
                    original_size,
                    conflict_size,
                    original_modified,
                    conflict_modified,
                });
            }
        }
    }

    Ok(())
}
