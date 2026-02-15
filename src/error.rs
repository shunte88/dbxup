/*
 *  error.rs
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
