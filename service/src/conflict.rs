//! Conflict detection and resolution for file synchronization
//!
//! When a file is modified on both the local filesystem and from the network
//! simultaneously, a conflict occurs. This module provides types and utilities
//! for detecting, representing, and resolving such conflicts.

use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    time::SystemTime,
};
use uuid::Uuid;

/// Information about a sync conflict
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConflictInfo {
    /// Unique identifier for this conflict
    pub id: Uuid,
    /// Path to the original file (relative to share root)
    pub original_path: PathBuf,
    /// Path to the conflicted version (with .sync-conflict suffix)
    pub conflict_path: PathBuf,
    /// When the conflict was detected
    pub detected_at: SystemTime,
    /// Size of the original file
    pub original_size: u64,
    /// Size of the conflicted file
    pub conflict_size: u64,
    /// Last modified time of the original file
    pub original_modified: SystemTime,
    /// Last modified time of the conflicted file
    pub conflict_modified: SystemTime,
}

/// Strategy for resolving a conflict
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ConflictResolution {
    /// Keep the local (original) version, delete the conflict
    KeepLocal,
    /// Keep the remote (conflict) version, replace the original
    KeepRemote,
    /// Keep both versions (do nothing, let user decide manually)
    KeepBoth,
    /// Delete both versions
    DeleteBoth,
}

/// Helper to generate a conflict filename
///
/// Converts `document.txt` to `document.sync-conflict-20251008-201530.txt`
pub fn generate_conflict_filename(original_path: &Path) -> PathBuf {
    let parent = original_path.parent();
    let filename = original_path.file_stem().and_then(|s| s.to_str());
    let extension = original_path.extension().and_then(|s| s.to_str());

    let timestamp = chrono::Local::now().format("%Y%m%d-%H%M%S");

    let conflict_name = match (filename, extension) {
        (Some(name), Some(ext)) => {
            format!("{}.sync-conflict-{}.{}", name, timestamp, ext)
        }
        (Some(name), None) => {
            format!("{}.sync-conflict-{}", name, timestamp)
        }
        _ => {
            format!("unnamed.sync-conflict-{}", timestamp)
        }
    };

    match parent {
        Some(p) => p.join(conflict_name),
        None => PathBuf::from(conflict_name),
    }
}

/// Check if a filename is a conflict file
pub fn is_conflict_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.contains(".sync-conflict-"))
        .unwrap_or(false)
}

/// Extract the original filename from a conflict filename
///
/// Converts `document.sync-conflict-20251008-201530.txt` to `document.txt`
pub fn get_original_from_conflict(conflict_path: &Path) -> Option<PathBuf> {
    let filename = conflict_path.file_name()?.to_str()?;

    // Split on ".sync-conflict-"
    if let Some(pos) = filename.find(".sync-conflict-") {
        let name_part = &filename[..pos];

        // Get extension after the timestamp
        let after_marker = &filename[pos + ".sync-conflict-".len()..];
        
        // Find the last dot for extension
        if let Some(dot_pos) = after_marker.rfind('.') {
            let ext = &after_marker[dot_pos + 1..];
            let original = format!("{}.{}", name_part, ext);
            
            return Some(conflict_path.with_file_name(original));
        } else {
            // No extension
            return Some(conflict_path.with_file_name(name_part));
        }
    }

    None
}

/// Parse conflict timestamp from filename
pub fn parse_conflict_timestamp(conflict_path: &Path) -> Option<chrono::NaiveDateTime> {
    let filename = conflict_path.file_name()?.to_str()?;

    // Find the timestamp portion: YYYYMMDD-HHMMSS
    if let Some(pos) = filename.find(".sync-conflict-") {
        let after_marker = &filename[pos + ".sync-conflict-".len()..];
        
        // Extract just the timestamp part (before the extension)
        let timestamp_str = if let Some(dot_pos) = after_marker.find('.') {
            &after_marker[..dot_pos]
        } else {
            after_marker
        };

        // Parse format: 20251008-201530
        if timestamp_str.len() >= 15 {
            let date_part = &timestamp_str[..8];  // YYYYMMDD
            let time_part = &timestamp_str[9..15]; // HHMMSS

            if let (Ok(year), Ok(month), Ok(day)) = (
                date_part[..4].parse::<i32>(),
                date_part[4..6].parse::<u32>(),
                date_part[6..8].parse::<u32>(),
            ) {
                if let (Ok(hour), Ok(minute), Ok(second)) = (
                    time_part[..2].parse::<u32>(),
                    time_part[2..4].parse::<u32>(),
                    time_part[4..6].parse::<u32>(),
                ) {
                    return chrono::NaiveDate::from_ymd_opt(year, month, day)
                        .and_then(|date| date.and_hms_opt(hour, minute, second));
                }
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_conflict_filename() {
        let original = PathBuf::from("document.txt");
        let conflict = generate_conflict_filename(&original);
        
        let name = conflict.file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with("document.sync-conflict-"));
        assert!(name.ends_with(".txt"));
    }

    #[test]
    fn test_generate_conflict_filename_no_extension() {
        let original = PathBuf::from("README");
        let conflict = generate_conflict_filename(&original);
        
        let name = conflict.file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with("README.sync-conflict-"));
    }

    #[test]
    fn test_is_conflict_file() {
        assert!(is_conflict_file(&PathBuf::from("doc.sync-conflict-20251008-201530.txt")));
        assert!(!is_conflict_file(&PathBuf::from("doc.txt")));
        assert!(!is_conflict_file(&PathBuf::from("doc.conflict.txt")));
    }

    #[test]
    fn test_get_original_from_conflict() {
        let conflict = PathBuf::from("document.sync-conflict-20251008-201530.txt");
        let original = get_original_from_conflict(&conflict).unwrap();
        assert_eq!(original, PathBuf::from("document.txt"));
    }

    #[test]
    fn test_get_original_from_conflict_no_extension() {
        let conflict = PathBuf::from("README.sync-conflict-20251008-201530");
        let original = get_original_from_conflict(&conflict).unwrap();
        assert_eq!(original, PathBuf::from("README"));
    }

    #[test]
    fn test_parse_conflict_timestamp() {
        let conflict = PathBuf::from("doc.sync-conflict-20251008-143025.txt");
        let timestamp = parse_conflict_timestamp(&conflict).unwrap();
        
        assert_eq!(timestamp.year(), 2025);
        assert_eq!(timestamp.month(), 10);
        assert_eq!(timestamp.day(), 8);
        assert_eq!(timestamp.hour(), 14);
        assert_eq!(timestamp.minute(), 30);
        assert_eq!(timestamp.second(), 25);
    }
}
