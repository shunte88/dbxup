/*
 *  persistent_queue.rs
 *
 *  dbxup - simple dropbox sync service
 *      (c) 2025-26 Stuart Hunter
 *
 *
 *      This program is free software: you can redistribute it and/or modify
 *      it under the terms of the GNU General Public License as published by
 *      the Free Software Foundation, either version 3 of the License, or
 *      (at your option) any later version.
 *
 *      This program is distributed in the hope that it will be useful,
 *      but WITHOUT ANY WARRANTY; without even the implied warranty of
 *      MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 *      GNU General Public License for more details.
 *
 *      See <http://www.gnu.org/licenses/> to get a copy of the GNU General
 *      Public License.
 *
 */

use crate::error::{DbxUpError, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Upload session info for resumable chunked uploads
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadSession {
    pub session_id: String,
    pub uploaded_bytes: u64,
    pub started_at: SystemTime,
}

/// Entry in the persistent upload queue
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueEntry {
    pub local_path: PathBuf,
    pub dropbox_path: String,
    pub size: u64,
    pub queued_at: SystemTime,
    pub upload_session: Option<UploadSession>,
    pub retry_count: u32,
}

impl QueueEntry {
    pub fn new(local_path: PathBuf, dropbox_path: String, size: u64) -> Self {
        Self {
            local_path,
            dropbox_path,
            size,
            queued_at: SystemTime::now(),
            upload_session: None,
            retry_count: 0,
        }
    }

    /// Check if the local file still exists and matches the queued size
    pub fn is_valid(&self) -> bool {
        match fs::metadata(&self.local_path) {
            Ok(meta) => meta.len() == self.size && meta.is_file(),
            Err(_) => false,
        }
    }

    /// Check if upload session is stale (>3 hours old, Dropbox sessions expire at 4 hours)
    pub fn is_session_stale(&self) -> bool {
        if let Some(session) = &self.upload_session {
            if let Ok(elapsed) = SystemTime::now().duration_since(session.started_at) {
                return elapsed > Duration::from_secs(3 * 3600); // 3 hours
            }
        }
        false
    }
}

/// Persistent upload queue that survives service restarts
pub struct PersistentQueue {
    entries: HashMap<PathBuf, QueueEntry>,
    queue_file: PathBuf,
}

impl PersistentQueue {
    /// Get default queue file path
    pub fn get_queue_file_path() -> PathBuf {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home).join(".dbxup_queue.json")
    }

    /// Create new persistent queue (loads from disk if exists)
    pub fn new() -> Result<Self> {
        let queue_file = Self::get_queue_file_path();
        let entries = if queue_file.exists() {
            Self::load_from_disk(&queue_file)?
        } else {
            HashMap::new()
        };

        Ok(Self {
            entries,
            queue_file,
        })
    }

    /// Load queue from disk
    fn load_from_disk(path: &Path) -> Result<HashMap<PathBuf, QueueEntry>> {
        let contents = fs::read_to_string(path).map_err(|e| {
            DbxUpError::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("Failed to read queue file: {}", e),
            ))
        })?;

        let entries: Vec<QueueEntry> = serde_json::from_str(&contents).map_err(|e| {
            DbxUpError::Config(format!("Failed to parse queue file: {}", e))
        })?;

        // Convert to HashMap and validate entries
        let mut map = HashMap::new();
        let mut valid_count = 0;
        let mut invalid_count = 0;

        for entry in entries {
            if entry.is_valid() {
                // Clear stale upload sessions
                let mut entry = entry;
                if entry.is_session_stale() {
                    log::info!(
                        "Clearing stale upload session for: {}",
                        entry.local_path.display()
                    );
                    entry.upload_session = None;
                }
                map.insert(entry.local_path.clone(), entry);
                valid_count += 1;
            } else {
                log::warn!(
                    "Removing invalid queue entry (file missing or changed): {}",
                    entry.local_path.display()
                );
                invalid_count += 1;
            }
        }

        log::info!(
            "Loaded persistent queue: {} valid entries, {} invalid entries removed",
            valid_count,
            invalid_count
        );

        Ok(map)
    }

    /// Save queue to disk
    pub fn save(&self) -> Result<()> {
        let entries: Vec<&QueueEntry> = self.entries.values().collect();
        let json = serde_json::to_string_pretty(&entries).map_err(|e| {
            DbxUpError::Config(format!("Failed to serialize queue: {}", e))
        })?;

        fs::write(&self.queue_file, json).map_err(|e| {
            DbxUpError::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("Failed to write queue file: {}", e),
            ))
        })?;

        log::debug!("Saved queue state: {} entries", self.entries.len());
        Ok(())
    }

    /// Add entry to queue
    pub fn add(&mut self, entry: QueueEntry) {
        self.entries.insert(entry.local_path.clone(), entry);
    }

    /// Remove entry from queue
    pub fn remove(&mut self, path: &Path) -> Option<QueueEntry> {
        self.entries.remove(path)
    }

    /// Get entry by path
    pub fn get(&self, path: &Path) -> Option<&QueueEntry> {
        self.entries.get(path)
    }

    /// Get mutable entry by path
    pub fn get_mut(&mut self, path: &Path) -> Option<&mut QueueEntry> {
        self.entries.get_mut(path)
    }

    /// Check if path is in queue
    pub fn contains(&self, path: &Path) -> bool {
        self.entries.contains_key(path)
    }

    /// Get all entries
    pub fn entries(&self) -> Vec<&QueueEntry> {
        self.entries.values().collect()
    }

    /// Get queue size
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Check if queue is empty
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Clear the entire queue
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Get entries that are ready to upload (no active session or stale session)
    pub fn get_ready_entries(&self) -> Vec<&QueueEntry> {
        self.entries
            .values()
            .filter(|e| e.upload_session.is_none() || e.is_session_stale())
            .collect()
    }

    /// Get entries with active upload sessions (for resume)
    pub fn get_resumable_entries(&self) -> Vec<&QueueEntry> {
        self.entries
            .values()
            .filter(|e| e.upload_session.is_some() && !e.is_session_stale())
            .collect()
    }

    /// Update upload session for an entry
    pub fn update_session(
        &mut self,
        path: &Path,
        session_id: String,
        uploaded_bytes: u64,
    ) -> Result<()> {
        if let Some(entry) = self.entries.get_mut(path) {
            entry.upload_session = Some(UploadSession {
                session_id,
                uploaded_bytes,
                started_at: SystemTime::now(),
            });
            Ok(())
        } else {
            Err(DbxUpError::Config(format!(
                "Path not in queue: {}",
                path.display()
            )))
        }
    }

    /// Increment retry count for an entry
    pub fn increment_retry(&mut self, path: &Path) {
        if let Some(entry) = self.entries.get_mut(path) {
            entry.retry_count += 1;
        }
    }

    /// Remove entries that have exceeded max retries
    pub fn prune_failed_entries(&mut self, max_retries: u32) -> Vec<QueueEntry> {
        let mut failed = Vec::new();

        self.entries.retain(|_, entry| {
            if entry.retry_count > max_retries {
                log::error!(
                    "Removing entry after {} failed attempts: {}",
                    entry.retry_count,
                    entry.local_path.display()
                );
                failed.push(entry.clone());
                false
            } else {
                true
            }
        });

        failed
    }
}

impl Drop for PersistentQueue {
    fn drop(&mut self) {
        // Auto-save on drop
        if let Err(e) = self.save() {
            log::error!("Failed to save queue on drop: {}", e);
        }
    }
}
