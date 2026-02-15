/*
 *  metadata.rs
 *
 *  dbxup - simple dropbox sync service
 *      (c) 2025-26 Stuart Hunter
 *
 *  Independent astronomical calculations (sunrise, sunset, moonrise, moonset)
 *  Used for auto-brightness and display - works without weather service
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
use std::path::{Path, PathBuf};

/// Metadata for a file to be uploaded
#[derive(Debug, Clone)]
pub struct FileMetadata {
    /// Local filesystem path
    pub local_path: PathBuf,
    /// Dropbox destination path (relative to destination root)
    pub dropbox_path: String,
    /// File size in bytes
    pub size: u64,
}

impl FileMetadata {
    /// Create new file metadata
    pub fn new(local_path: PathBuf, dropbox_path: String, size: u64) -> Self {
        Self {
            local_path,
            dropbox_path,
            size,
        }
    }

    /// Check if file requires chunked upload (>= 150MB)
    pub fn requires_chunked_upload(&self) -> bool {
        self.size >= 150 * 1024 * 1024
    }

    /// Check if file exceeds Dropbox's 350GB limit
    pub fn exceeds_dropbox_limit(&self) -> bool {
        self.size > 350 * 1024 * 1024 * 1024
    }
}

/// Maps local file path to Dropbox path
pub struct PathMapper {
    base_local_path: PathBuf,
    base_dropbox_path: String,
}

impl PathMapper {
    /// Create new path mapper
    pub fn new(base_local_path: PathBuf, base_dropbox_path: String) -> Self {
        Self {
            base_local_path,
            base_dropbox_path,
        }
    }

    /// Map a local file path to its Dropbox path
    pub fn map_to_dropbox(&self, local_path: &Path) -> Result<String> {
        // For single files, use the base dropbox path + filename
        if local_path == self.base_local_path {
            let filename = local_path
                .file_name()
                .ok_or_else(|| DbxUpError::InvalidPath("No filename found".to_string()))?
                .to_string_lossy();

            // Ensure dropbox path ends with /
            let base = if self.base_dropbox_path.ends_with('/') {
                &self.base_dropbox_path
            } else {
                return Ok(format!("{}/{}", self.base_dropbox_path, filename));
            };
            return Ok(format!("{}{}", base, filename));
        }

        // For directory uploads, preserve relative structure
        let relative = local_path
            .strip_prefix(&self.base_local_path)
            .map_err(|_| {
                DbxUpError::InvalidPath(format!(
                    "Path {} is not within base path {}",
                    local_path.display(),
                    self.base_local_path.display()
                ))
            })?;

        // Convert to string and replace Windows separators
        let relative_str = relative.to_string_lossy().replace('\\', "/");

        // Combine with base dropbox path
        let dropbox_path = if self.base_dropbox_path.ends_with('/') {
            format!("{}{}", self.base_dropbox_path, relative_str)
        } else {
            format!("{}/{}", self.base_dropbox_path, relative_str)
        };

        Ok(dropbox_path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunked_upload_threshold() {
        let small_file = FileMetadata::new(
            PathBuf::from("/test.txt"),
            "/test.txt".to_string(),
            100 * 1024 * 1024, // 100MB
        );
        assert!(!small_file.requires_chunked_upload());

        let large_file = FileMetadata::new(
            PathBuf::from("/large.txt"),
            "/large.txt".to_string(),
            200 * 1024 * 1024, // 200MB
        );
        assert!(large_file.requires_chunked_upload());
    }

    #[test]
    fn test_dropbox_limit() {
        let huge_file = FileMetadata::new(
            PathBuf::from("/huge.bin"),
            "/huge.bin".to_string(),
            400 * 1024 * 1024 * 1024u64, // 400GB
        );
        assert!(huge_file.exceeds_dropbox_limit());
    }
}
