/*
 *  client.rs
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

us
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
}
