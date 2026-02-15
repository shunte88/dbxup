use std::path::PathBuf;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum DbxUpError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Dropbox API error: {0}")]
    DropboxApi(String),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Authentication failed: {0}")]
    Authentication(String),

    #[error("File not found: {0}")]
    FileNotFound(PathBuf),

    #[error("Invalid path: {0}")]
    InvalidPath(String),

    #[error("File too large: {0} exceeds Dropbox limit of 350GB")]
    FileTooLarge(PathBuf),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Upload failed after {retries} retries: {message}")]
    UploadFailed { retries: u32, message: String },
}

pub type Result<T> = std::result::Result<T, DbxUpError>;
