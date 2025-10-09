//! Share persistence integration
//!
//! This module handles loading shares on State initialization
//! and provides helper functions for persisting changes.

use crate::{
    error::Error,
    share::{ShareHolder, ShareInfo},
    share_config::{ShareConfigEntry, SharesConfig},
    sync_bridge::SyncBridge,
};
use ouisync::{AccessSecrets, Credentials, Repository, RepositoryParams};
use std::{path::Path, sync::Arc};
use tracing::{debug, error, warn};

impl super::State {
    /// Load all shares from configuration
    pub(super) async fn load_shares(&self) {
        debug!("Loading shares from configuration");

        // Load config
        let config = match SharesConfig::load(self.config.dir()).await {
            Ok(config) => config,
            Err(e) => {
                error!("Failed to load shares config: {:?}", e);
                return;
            }
        };

        if config.shares.is_empty() {
            debug!("No shares in configuration");
            return;
        }

        debug!("Found {} shares in configuration", config.shares.len());

        // Load each share
        for entry in config.shares {
            match self.load_share_from_config(entry).await {
                Ok(()) => {}
                Err(e) => {
                    error!("Failed to load share: {:?}", e);
                }
            }
        }

        debug!("Finished loading shares");
    }

    /// Load a single share from config entry
    async fn load_share_from_config(&self, entry: ShareConfigEntry) -> Result<(), Error> {
        debug!("Loading share {} at {:?}", entry.id, entry.path);

        // Check if directory exists
        if !tokio::fs::try_exists(&entry.path)
            .await
            .unwrap_or(false)
        {
            warn!(
                "Share {} directory does not exist: {:?}",
                entry.id, entry.path
            );
            return Ok(()); // Skip but don't fail
        }

        // Repository path is inside the directory
        let repo_path = entry.path.join(".ouisyncdb");

        // Check if repository exists
        if !tokio::fs::try_exists(&repo_path)
            .await
            .unwrap_or(false)
        {
            warn!("Share {} repository does not exist: {:?}", entry.id, repo_path);
            return Ok(()); // Skip but don't fail
        }

        // Open existing repository
        let params = RepositoryParams::new(repo_path.clone());

        // Try to open without credentials first (will fail if encrypted)
        let repository = match Repository::open(&params, Credentials::None).await {
            Ok(repo) => Arc::new(repo),
            Err(e) => {
                error!(
                    "Failed to open repository for share {}: {:?}",
                    entry.id, e
                );
                return Ok(()); // Skip but don't fail
            }
        };

        // Register with network
        let registration = self.network.register(
            repository.handle(),
            self.repos_monitor.make_child(&entry.name),
        );

        // Set sync enabled state
        registration.set_sync_enabled(entry.sync_enabled);

        // Add peers to network
        for peer in &entry.peers {
            self.network.add_user_provided_peer(peer);
        }

        // Create sync bridge if sync is enabled
        let sync_bridge = if entry.sync_enabled {
            match SyncBridge::new(entry.path.clone(), repository.clone()).await {
                Ok(bridge) => Some(bridge),
                Err(e) => {
                    error!("Failed to create sync bridge for share {}: {:?}", entry.id, e);
                    None
                }
            }
        } else {
            None
        };

        // Create ShareInfo
        let share_info = ShareInfo {
            id: entry.id,
            path: entry.path,
            name: entry.name,
            peers: entry.peers,
            sync_enabled: entry.sync_enabled,
            repository_id: *repository.secrets().id(),
        };

        // Create and insert ShareHolder
        let holder = ShareHolder::new(share_info, repository, sync_bridge);

        match self.shares.try_insert(holder) {
            Ok(_) => {
                debug!("Successfully loaded share {}", entry.id);
            }
            Err(e) => {
                error!("Failed to insert share {}: {:?}", entry.id, e);
            }
        }

        Ok(())
    }

    /// Save current shares to configuration
    pub(super) async fn save_shares(&self) -> Result<(), Error> {
        let shares = self.shares.list();
        let config = SharesConfig::from(shares);
        config.save(self.config.dir()).await
    }
}
