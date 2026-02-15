/*
 *  token_storage.rs
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
use std::fs;
use std::io;
use std::path::PathBuf;

/// Token storage with refresh token support
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TokenStore {
    pub app_key: String,
    pub refresh_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_in: Option<i64>,
}

impl TokenStore {
    pub fn new(app_key: String, refresh_token: String) -> Self {
        Self {
            app_key,
            refresh_token,
            access_token: None,
            expires_in: None,
        }
    }

    /// Get path to token storage file
    pub fn get_path() -> PathBuf {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home).join(".dbxup_tokens")
    }

    /// Save token store to file
    pub fn save(&self) -> Result<()> {
        let path = Self::get_path();
        let json = serde_json::to_string_pretty(self).map_err(|e| {
            DbxUpError::Config(format!("Failed to serialize tokens: {}", e))
        })?;

        fs::write(&path, json).map_err(|e| {
            DbxUpError::Io(io::Error::new(
                io::ErrorKind::Other,
                format!("Failed to save tokens: {}", e),
            ))
        })?;

        println!("💾 Tokens saved to: {}", path.display());
        Ok(())
    }

    /// Load token store from file
    pub fn load() -> Result<Self> {
        let path = Self::get_path();

        if !path.exists() {
            return Err(DbxUpError::Config(
                "No saved tokens found. Run 'dbxup auth' to set up.".to_string(),
            ));
        }

        let contents = fs::read_to_string(&path).map_err(|e| {
            DbxUpError::Io(io::Error::new(
                io::ErrorKind::Other,
                format!("Failed to read tokens: {}", e),
            ))
        })?;

        serde_json::from_str(&contents).map_err(|e| {
            DbxUpError::Config(format!("Failed to parse tokens: {}", e))
        })
    }

    /// Clear saved tokens
    pub fn clear() -> Result<()> {
        let path = Self::get_path();

        if path.exists() {
            fs::remove_file(&path).map_err(|e| {
                DbxUpError::Io(io::Error::new(
                    io::ErrorKind::Other,
                    format!("Failed to delete tokens: {}", e),
                ))
            })?;
            println!("🗑️  Tokens cleared from: {}", path.display());
        } else {
            println!("No saved tokens found.");
        }

        Ok(())
    }
}
