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
