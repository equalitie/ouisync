//! Share configuration persistence
//!
//! This module handles saving and loading share configurations to/from disk,
//! ensuring shares survive application restarts.

use crate::{error::Error, share::ShareInfo};
use ouisync::PeerAddr;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::fs;
use tracing::{debug, error, warn};
use uuid::Uuid;

const SHARES_CONFIG_FILENAME: &str = "shares.json";

/// Configuration for all shares
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharesConfig {
    /// Version of the config format (for future migrations)
    #[serde(default = "default_version")]
    pub version: u32,
    /// List of configured shares
    pub shares: Vec<ShareConfigEntry>,
}

fn default_version() -> u32 {
    1
}

/// Configuration entry for a single share
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShareConfigEntry {
    /// Unique identifier
    pub id: Uuid,
    /// Absolute path to the local directory
    pub path: PathBuf,
    /// User-friendly name
    pub name: String,
    /// List of peer addresses
    pub peers: Vec<PeerAddr>,
    /// Whether sync is enabled
    #[serde(default = "default_sync_enabled")]
    pub sync_enabled: bool,
}

fn default_sync_enabled() -> bool {
    true
}

impl From<ShareInfo> for ShareConfigEntry {
    fn from(info: ShareInfo) -> Self {
        Self {
            id: info.id,
            path: info.path,
            name: info.name,
            peers: info.peers,
            sync_enabled: info.sync_enabled,
        }
    }
}

impl SharesConfig {
    /// Create a new empty configuration
    pub fn new() -> Self {
        Self {
            version: 1,
            shares: Vec::new(),
        }
    }

    /// Load configuration from disk
    ///
    /// # Arguments
    /// * `config_dir` - Directory containing the configuration file
    ///
    /// # Returns
    /// * `Ok(SharesConfig)` - Loaded configuration
    /// * `Err(Error)` - If file doesn't exist or is malformed
    pub async fn load(config_dir: &Path) -> Result<Self, Error> {
        let path = config_dir.join(SHARES_CONFIG_FILENAME);

        debug!("Loading shares config from {:?}", path);

        if !fs::try_exists(&path).await.unwrap_or(false) {
            debug!("Shares config file does not exist, returning empty config");
            return Ok(Self::new());
        }

        let content = fs::read_to_string(&path)
            .await
            .map_err(|e| Error::Other(format!("Failed to read shares config: {}", e)))?;

        let config: SharesConfig = serde_json::from_str(&content)
            .map_err(|e| Error::Other(format!("Failed to parse shares config: {}", e)))?;

        debug!("Loaded {} shares from config", config.shares.len());

        // Validate configuration
        config.validate()?;

        Ok(config)
    }

    /// Save configuration to disk atomically
    ///
    /// Uses a temporary file and atomic rename to prevent corruption.
    ///
    /// # Arguments
    /// * `config_dir` - Directory to save the configuration file
    pub async fn save(&self, config_dir: &Path) -> Result<(), Error> {
        // Ensure config directory exists
        fs::create_dir_all(config_dir)
            .await
            .map_err(|e| Error::Other(format!("Failed to create config directory: {}", e)))?;

        let path = config_dir.join(SHARES_CONFIG_FILENAME);
        let temp_path = config_dir.join(format!("{}.tmp", SHARES_CONFIG_FILENAME));

        debug!("Saving shares config to {:?}", path);

        // Serialize to JSON with pretty formatting
        let content = serde_json::to_string_pretty(self)
            .map_err(|e| Error::Other(format!("Failed to serialize shares config: {}", e)))?;

        // Write to temporary file
        fs::write(&temp_path, content)
            .await
            .map_err(|e| Error::Other(format!("Failed to write shares config: {}", e)))?;

        // Atomic rename
        fs::rename(&temp_path, &path)
            .await
            .map_err(|e| Error::Other(format!("Failed to rename shares config: {}", e)))?;

        debug!("Saved {} shares to config", self.shares.len());

        Ok(())
    }

    /// Add a share to the configuration
    pub fn add_share(&mut self, entry: ShareConfigEntry) -> Result<(), Error> {
        // Check for duplicate ID
        if self.shares.iter().any(|s| s.id == entry.id) {
            return Err(Error::Other(format!(
                "Share with ID {} already exists in config",
                entry.id
            )));
        }

        // Check for duplicate path
        if self.shares.iter().any(|s| s.path == entry.path) {
            return Err(Error::Other(format!(
                "Share with path {:?} already exists in config",
                entry.path
            )));
        }

        self.shares.push(entry);
        Ok(())
    }

    /// Remove a share from the configuration
    pub fn remove_share(&mut self, share_id: Uuid) -> Result<(), Error> {
        let len_before = self.shares.len();
        self.shares.retain(|s| s.id != share_id);

        if self.shares.len() == len_before {
            return Err(Error::Other(format!(
                "Share with ID {} not found in config",
                share_id
            )));
        }

        Ok(())
    }

    /// Update a share in the configuration
    pub fn update_share(&mut self, entry: ShareConfigEntry) -> Result<(), Error> {
        let share = self
            .shares
            .iter_mut()
            .find(|s| s.id == entry.id)
            .ok_or_else(|| {
                Error::Other(format!("Share with ID {} not found in config", entry.id))
            })?;

        *share = entry;
        Ok(())
    }

    /// Get a share by ID
    pub fn get_share(&self, share_id: Uuid) -> Option<&ShareConfigEntry> {
        self.shares.iter().find(|s| s.id == share_id)
    }

    /// Validate the configuration
    fn validate(&self) -> Result<(), Error> {
        // Check for duplicate IDs
        let mut ids = std::collections::HashSet::new();
        for share in &self.shares {
            if !ids.insert(share.id) {
                return Err(Error::Other(format!(
                    "Duplicate share ID in config: {}",
                    share.id
                )));
            }
        }

        // Check for duplicate paths
        let mut paths = std::collections::HashSet::new();
        for share in &self.shares {
            if !paths.insert(&share.path) {
                return Err(Error::Other(format!(
                    "Duplicate share path in config: {:?}",
                    share.path
                )));
            }
        }

        // Validate each share
        for share in &self.shares {
            if !share.path.is_absolute() {
                warn!(
                    "Share {} has non-absolute path: {:?}",
                    share.id, share.path
                );
            }

            if share.name.is_empty() {
                return Err(Error::Other(format!(
                    "Share {} has empty name",
                    share.id
                )));
            }
        }

        Ok(())
    }
}

impl Default for SharesConfig {
    fn default() -> Self {
        Self::new()
    }
}

/// Helper to create SharesConfig from ShareInfo list
impl From<Vec<ShareInfo>> for SharesConfig {
    fn from(shares: Vec<ShareInfo>) -> Self {
        Self {
            version: 1,
            shares: shares.into_iter().map(ShareConfigEntry::from).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_save_and_load() {
        let temp_dir = TempDir::new().unwrap();
        let config_dir = temp_dir.path();

        // Create a config with some shares
        let mut config = SharesConfig::new();
        config
            .add_share(ShareConfigEntry {
                id: Uuid::new_v4(),
                path: PathBuf::from("/tmp/share1"),
                name: "Share 1".to_string(),
                peers: vec![],
                sync_enabled: true,
            })
            .unwrap();

        // Save
        config.save(config_dir).await.unwrap();

        // Load
        let loaded = SharesConfig::load(config_dir).await.unwrap();

        assert_eq!(loaded.shares.len(), 1);
        assert_eq!(loaded.shares[0].name, "Share 1");
    }

    #[tokio::test]
    async fn test_load_nonexistent() {
        let temp_dir = TempDir::new().unwrap();
        let config_dir = temp_dir.path();

        // Load from empty directory should return empty config
        let config = SharesConfig::load(config_dir).await.unwrap();
        assert_eq!(config.shares.len(), 0);
    }

    #[tokio::test]
    async fn test_add_duplicate() {
        let mut config = SharesConfig::new();
        let id = Uuid::new_v4();

        config
            .add_share(ShareConfigEntry {
                id,
                path: PathBuf::from("/tmp/share1"),
                name: "Share 1".to_string(),
                peers: vec![],
                sync_enabled: true,
            })
            .unwrap();

        // Try to add duplicate ID
        let result = config.add_share(ShareConfigEntry {
            id, // Same ID
            path: PathBuf::from("/tmp/share2"),
            name: "Share 2".to_string(),
            peers: vec![],
            sync_enabled: true,
        });

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_remove_share() {
        let mut config = SharesConfig::new();
        let id = Uuid::new_v4();

        config
            .add_share(ShareConfigEntry {
                id,
                path: PathBuf::from("/tmp/share1"),
                name: "Share 1".to_string(),
                peers: vec![],
                sync_enabled: true,
            })
            .unwrap();

        assert_eq!(config.shares.len(), 1);

        config.remove_share(id).unwrap();
        assert_eq!(config.shares.len(), 0);
    }

    #[tokio::test]
    async fn test_update_share() {
        let mut config = SharesConfig::new();
        let id = Uuid::new_v4();

        config
            .add_share(ShareConfigEntry {
                id,
                path: PathBuf::from("/tmp/share1"),
                name: "Share 1".to_string(),
                peers: vec![],
                sync_enabled: true,
            })
            .unwrap();

        // Update
        config
            .update_share(ShareConfigEntry {
                id,
                path: PathBuf::from("/tmp/share1"),
                name: "Updated Share".to_string(),
                peers: vec![],
                sync_enabled: false,
            })
            .unwrap();

        let share = config.get_share(id).unwrap();
        assert_eq!(share.name, "Updated Share");
        assert!(!share.sync_enabled);
    }

    #[tokio::test]
    async fn test_atomic_save() {
        let temp_dir = TempDir::new().unwrap();
        let config_dir = temp_dir.path();

        let config = SharesConfig::new();
        config.save(config_dir).await.unwrap();

        // Verify temp file doesn't exist
        let temp_path = config_dir.join("shares.json.tmp");
        assert!(!temp_path.exists());

        // Verify actual file exists
        let actual_path = config_dir.join("shares.json");
        assert!(actual_path.exists());
    }
}
