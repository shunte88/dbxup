/*
 *  scanner.rs
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
use crate::files::metadata::{FileMetadata, PathMapper};
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Scanner for discovering files to upload
pub struct FileScanner {
    mapper: PathMapper,
    skip_hidden: bool,
}

impl FileScanner {
    /// Create a new file scanner
    pub fn new(base_local_path: PathBuf, base_dropbox_path: String) -> Self {
        Self {
            mapper: PathMapper::new(base_local_path, base_dropbox_path),
            skip_hidden: true,
        }
    }

    /// Scan a single file
    pub fn scan_file(&self, path: &Path) -> Result<Vec<FileMetadata>> {
        if !path.exists() {
            return Err(DbxUpError::FileNotFound(path.to_path_buf()));
        }

        if !path.is_file() {
            return Err(DbxUpError::InvalidPath(format!(
                "{} is not a file",
                path.display()
            )));
        }

        let metadata = fs::metadata(path)?;
        let size = metadata.len();

        // Check if file is too large
        if size > 350 * 1024 * 1024 * 1024 {
            return Err(DbxUpError::FileTooLarge(path.to_path_buf()));
        }

        let dropbox_path = self.mapper.map_to_dropbox(path)?;
        let file_meta = FileMetadata::new(path.to_path_buf(), dropbox_path, size);

        Ok(vec![file_meta])
    }

    /// Scan a directory recursively
    pub fn scan_directory(&self, path: &Path) -> Result<Vec<FileMetadata>> {
        if !path.exists() {
            return Err(DbxUpError::FileNotFound(path.to_path_buf()));
        }

        if !path.is_dir() {
            return Err(DbxUpError::InvalidPath(format!(
                "{} is not a directory",
                path.display()
            )));
        }

        let mut files = Vec::new();
        let mut errors = Vec::new();

        for entry in WalkDir::new(path)
            .follow_links(false) // Don't follow symlinks to prevent loops
            .into_iter()
        {
            match entry {
                Ok(entry) => {
                    let entry_path = entry.path();

                    // Skip directories
                    if entry_path.is_dir() {
                        continue;
                    }

                    // Skip symlinks
                    if entry.file_type().is_symlink() {
                        continue;
                    }

                    // Skip hidden files if configured
                    if self.skip_hidden && self.is_hidden(entry_path) {
                        continue;
                    }

                    // Get file metadata
                    match fs::metadata(entry_path) {
                        Ok(metadata) => {
                            let size = metadata.len();

                            // Skip if file exceeds Dropbox limit
                            if size > 350 * 1024 * 1024 * 1024 {
                                errors.push(format!(
                                    "Skipping {}: exceeds 350GB limit",
                                    entry_path.display()
                                ));
                                continue;
                            }

                            // Map to Dropbox path
                            match self.mapper.map_to_dropbox(entry_path) {
                                Ok(dropbox_path) => {
                                    let file_meta = FileMetadata::new(
                                        entry_path.to_path_buf(),
                                        dropbox_path,
                                        size,
                                    );
                                    files.push(file_meta);
                                }
                                Err(e) => {
                                    errors.push(format!(
                                        "Failed to map path {}: {}",
                                        entry_path.display(),
                                        e
                                    ));
                                }
                            }
                        }
                        Err(e) => {
                            errors.push(format!(
                                "Failed to read metadata for {}: {}",
                                entry_path.display(),
                                e
                            ));
                        }
                    }
                }
                Err(e) => {
                    errors.push(format!("Error walking directory: {}", e));
                }
            }
        }

        // Print errors if any
        if !errors.is_empty() {
            eprintln!("Warnings during directory scan:");
            for error in &errors {
                eprintln!("  {}", error);
            }
        }

        if files.is_empty() {
            return Err(DbxUpError::Config(
                "No files found to upload".to_string(),
            ));
        }

        Ok(files)
    }

    /// Check if a file/directory is hidden
    fn is_hidden(&self, path: &Path) -> bool {
        path.file_name()
            .and_then(|name| name.to_str())
            .map(|name| name.starts_with('.'))
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_hidden() {
        let scanner = FileScanner::new(PathBuf::from("/test"), "/dest".to_string());

        assert!(scanner.is_hidden(Path::new("/path/.hidden")));
        assert!(scanner.is_hidden(Path::new(".git")));
        assert!(!scanner.is_hidden(Path::new("/path/visible.txt")));
    }
}
