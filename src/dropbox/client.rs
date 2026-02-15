/*
 *  client.rs
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
use dropbox_sdk::{
    default_async_client::UserAuthDefaultClient, oauth2::Authorization, users,
};

/// Wrapper for Dropbox API client
pub struct DropboxClient {
    client: UserAuthDefaultClient,
}

impl DropboxClient {
    /// Create a new Dropbox client with the given access token
    pub fn new(token: String) -> Self {
        let auth = Authorization::from_long_lived_access_token(token);
        let client = UserAuthDefaultClient::new(auth);
        Self { client }
    }

    /// Create a new Dropbox client from an Authorization object
    pub fn from_authorization(auth: Authorization) -> Self {
        let client = UserAuthDefaultClient::new(auth);
        Self { client }
    }

    /// Validate the access token by fetching current account info
    pub async fn validate_token(&self) -> Result<()> {
        users::get_current_account(&self.client)
            .await
            .map_err(|e| DbxUpError::Authentication(format!("Invalid token: {}", e)))?;
        Ok(())
    }

    /// Get the inner client for API calls
    pub fn client(&self) -> &UserAuthDefaultClient {
        &self.client
    }

    /// Check if a file exists at the given Dropbox path and return its size
    /// Returns Some(size) if file exists, None if it doesn't exist
    pub async fn get_file_size(&self, path: &str) -> Result<Option<u64>> {
        use dropbox_sdk::files;

        match files::get_metadata(&self.client, &files::GetMetadataArg::new(path.to_string())).await {
            Ok(metadata) => {
                // Extract size from metadata
                if let files::Metadata::File(file_metadata) = metadata {
                    Ok(Some(file_metadata.size))
                } else {
                    // It's a folder, not a file
                    Ok(None)
                }
            }
            Err(e) => {
                // Check if it's a "not found" error
                let error_str = e.to_string();
                if error_str.contains("not_found") || error_str.contains("path/not_found") {
                    Ok(None)
                } else {
                    // Other error - propagate it
                    Err(DbxUpError::DropboxApi(format!("Failed to get file metadata: {}", e)))
                }
            }
        }
    }
}
