use crate::error::{DbxUpError, Result};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
    pub token: String,
    pub source: UploadSource,
    pub destination: String,
    pub parallel: usize,
    pub chunk_size_mb: usize,
    pub verbose: bool,
}

#[derive(Debug, Clone)]
pub enum UploadSource {
    File(PathBuf),
    Directory(PathBuf),
}

impl Config {
    pub fn validate(self) -> Result<Self> {
        // Validate source path exists
        let source_path = match &self.source {
            UploadSource::File(path) => path,
            UploadSource::Directory(path) => path,
        };

        if !source_path.exists() {
            return Err(DbxUpError::FileNotFound(source_path.clone()));
        }

        // Validate destination path format (must start with /)
        if !self.destination.starts_with('/') {
            return Err(DbxUpError::InvalidPath(
                "Destination path must start with '/'".to_string(),
            ));
        }

        // Validate parallelism is reasonable
        if self.parallel == 0 {
            return Err(DbxUpError::Config(
                "Parallel count must be at least 1".to_string(),
            ));
        }

        if self.parallel > 100 {
            return Err(DbxUpError::Config(
                "Parallel count cannot exceed 100".to_string(),
            ));
        }

        // Validate chunk size
        if self.chunk_size_mb == 0 {
            return Err(DbxUpError::Config(
                "Chunk size must be at least 1MB".to_string(),
            ));
        }

        Ok(self)
    }
}
