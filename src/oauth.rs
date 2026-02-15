/*
 *  oauth.rs
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
use dropbox_sdk::oauth2::{Authorization, Oauth2Type};
use http_body_util::Full;
use hyper::body::Bytes;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use std::fs;
use std::io::{self, Write};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use url::Url;

const REDIRECT_URI: &str = "http://localhost:8888/oauth/callback";

/// OAuth configuration
pub struct OAuthConfig {
    pub app_key: String,
    pub app_secret: String,
}

impl OAuthConfig {
    pub fn new(app_key: String, app_secret: String) -> Self {
        Self {
            app_key,
            app_secret,
        }
    }
}

/// Perform full OAuth 2.0 flow with refresh token support
pub async fn perform_oauth_flow(config: &OAuthConfig) -> Result<Authorization> {
    println!("\n🔐 Starting OAuth 2.0 authorization flow...\n");

    // Generate authorization URL
    let auth_url = generate_auth_url(&config.app_key, REDIRECT_URI);

    println!("Please visit this URL to authorize the application:");
    println!("\n  {}\n", auth_url);
    println!("Waiting for authorization...");

    // Try to open the URL in the browser
    if let Err(_) = open_browser(&auth_url) {
        println!("(Could not open browser automatically - please copy the URL above)");
    }

    // Start local HTTP server to receive callback
    let auth_code = receive_oauth_callback().await?;

    println!("\n🔄 Exchanging authorization code for access token...");

    // Exchange code for token with refresh token support
    let mut auth = Authorization::from_auth_code(
        config.app_key.clone(),
        Oauth2Type::AuthorizationCode {
            client_secret: config.app_secret.clone(),
        },
        auth_code,
        Some(REDIRECT_URI.to_string()),
    );

    // Make an API call to materialize the token (this forces the exchange)
    // We'll validate by getting current account info
    use dropbox_sdk::{default_async_client::UserAuthDefaultClient, users};
    let client = UserAuthDefaultClient::new(auth.clone());

    users::get_current_account(&client).await.map_err(|e| {
        DbxUpError::Config(format!("Failed to validate authorization: {}", e))
    })?;

    println!("✓ Authorization successful! Token will auto-refresh when needed.\n");

    Ok(auth)
}

/// Generate authorization URL with offline access for refresh token
fn generate_auth_url(app_key: &str, redirect_uri: &str) -> String {
    format!(
        "https://www.dropbox.com/oauth2/authorize?client_id={}&response_type=code&redirect_uri={}&token_access_type=offline",
        app_key,
        urlencoding::encode(redirect_uri)
    )
}

/// Open URL in the default browser
fn open_browser(url: &str) -> std::result::Result<(), Box<dyn std::error::Error>> {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(&["/C", "start", url])
            .spawn()?;
    }

    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(url).spawn()?;
    }

    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open").arg(url).spawn()?;
    }

    Ok(())
}

/// Start local HTTP server and wait for OAuth callback
async fn receive_oauth_callback() -> Result<String> {
    let addr: SocketAddr = "127.0.0.1:8888".parse().unwrap();
    let listener = TcpListener::bind(addr).await.map_err(|e| {
        DbxUpError::Config(format!(
            "Failed to start OAuth callback server on {}: {}. Make sure port 8080 is available.",
            addr, e
        ))
    })?;

    println!("📡 Listening for OAuth callback on {}...", addr);

    let auth_code = Arc::new(Mutex::new(None));
    let auth_code_clone = auth_code.clone();

    // Accept single connection
    let (stream, _) = listener.accept().await.map_err(|e| {
        DbxUpError::Config(format!("Failed to accept connection: {}", e))
    })?;

    let io = hyper_util::rt::TokioIo::new(stream);

    // Create service to handle request
    let service = service_fn(move |req: Request<hyper::body::Incoming>| {
        let auth_code = auth_code_clone.clone();
        async move {
            handle_callback(req, auth_code).await
        }
    });

    // Handle the connection
    if let Err(e) = http1::Builder::new().serve_connection(io, service).await {
        return Err(DbxUpError::Config(format!("Server error: {}", e)));
    }

    // Extract the authorization code
    let code = auth_code.lock().await.take().ok_or_else(|| {
        DbxUpError::Config("No authorization code received".to_string())
    })?;

    Ok(code)
}

/// Handle OAuth callback request
async fn handle_callback(
    req: Request<hyper::body::Incoming>,
    auth_code: Arc<Mutex<Option<String>>>,
) -> std::result::Result<Response<Full<Bytes>>, hyper::Error> {
    let uri = req.uri().to_string();

    // Parse query parameters
    if let Ok(url) = Url::parse(&format!("http://localhost{}", uri)) {
        let params: std::collections::HashMap<_, _> = url.query_pairs().collect();

        if let Some(code) = params.get("code") {
            // Store the authorization code
            *auth_code.lock().await = Some(code.to_string());

            // Send success response
            let html = r#"
<!DOCTYPE html>
<html>
<head>
    <title>Authorization Successful</title>
    <style>
        body { font-family: Arial, sans-serif; text-align: center; padding: 50px; }
        .success { color: #28a745; font-size: 24px; }
        .message { margin-top: 20px; color: #666; }
    </style>
</head>
<body>
    <div class="success">✓ Authorization Successful!</div>
    <div class="message">You can close this window and return to the terminal.</div>
</body>
</html>
"#;
            return Ok(Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", "text/html")
                .body(Full::new(Bytes::from(html)))
                .unwrap());
        } else if let Some(error) = params.get("error") {
            // Handle error
            let html = format!(
                r#"
<!DOCTYPE html>
<html>
<head>
    <title>Authorization Failed</title>
    <style>
        body {{ font-family: Arial, sans-serif; text-align: center; padding: 50px; }}
        .error {{ color: #dc3545; font-size: 24px; }}
        .message {{ margin-top: 20px; color: #666; }}
    </style>
</head>
<body>
    <div class="error">✗ Authorization Failed</div>
    <div class="message">Error: {}</div>
    <div class="message">Please close this window and try again.</div>
</body>
</html>
"#,
                error
            );
            return Ok(Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .header("Content-Type", "text/html")
                .body(Full::new(Bytes::from(html)))
                .unwrap());
        }
    }

    // Fallback response
    Ok(Response::builder()
        .status(StatusCode::BAD_REQUEST)
        .body(Full::new(Bytes::from("Invalid request")))
        .unwrap())
}

/// Get token cache file path
pub fn get_token_cache_path() -> PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".dbxup_token")
}

/// Save authorization to cache file
pub fn save_token(auth: &Authorization) -> Result<()> {
    let cache_path = get_token_cache_path();
    let serialized = auth.save().ok_or_else(|| {
        DbxUpError::Config("Failed to serialize authorization".to_string())
    })?;
    fs::write(&cache_path, serialized).map_err(|e| {
        DbxUpError::Io(io::Error::new(
            io::ErrorKind::Other,
            format!("Failed to save token: {}", e),
        ))
    })?;
    println!("💾 Token saved to: {}", cache_path.display());
    println!("   (includes refresh token for automatic renewal)");
    Ok(())
}

/// Load authorization from cache file
pub fn load_token(app_key: &str) -> Result<Authorization> {
    let cache_path = get_token_cache_path();

    if !cache_path.exists() {
        return Err(DbxUpError::Config(
            "No saved token found. Please run 'dbxup auth' to authorize.".to_string(),
        ));
    }

    let serialized = fs::read_to_string(&cache_path).map_err(|e| {
        DbxUpError::Io(io::Error::new(
            io::ErrorKind::Other,
            format!("Failed to load token: {}", e),
        ))
    })?;

    Authorization::load(app_key.to_string(), &serialized).ok_or_else(|| {
        DbxUpError::Config(
            "Failed to parse saved token. Please run 'dbxup auth' to re-authorize.".to_string(),
        )
    })
}

/// Set up authorization with refresh token (simpler method)
pub async fn setup_with_refresh_token(
    app_key: String,
    refresh_token: String,
) -> Result<Authorization> {
    println!("\n🔐 Setting up authorization with refresh token...\n");

    // Create authorization from refresh token
    let auth = Authorization::from_refresh_token(app_key.clone(), refresh_token);

    // Validate by making an API call (forces token fetch)
    use dropbox_sdk::{default_async_client::UserAuthDefaultClient, users};
    let client = UserAuthDefaultClient::new(auth.clone());

    let account = users::get_current_account(&client).await.map_err(|e| {
        DbxUpError::Authentication(format!("Failed to validate refresh token: {}", e))
    })?;

    println!("✓ Authenticated as: {}", account.name.display_name);
    println!("✓ Token will automatically refresh when needed\n");

    Ok(auth)
}

/// Simple CLI OAuth flow - user provides authorization code
pub async fn simple_oauth_flow(app_key: String, app_secret: String) -> Result<Authorization> {
    println!("\n🔐 Dropbox Authorization (Simple Method)\n");

    // Generate authorization URL
    let auth_url = format!(
        "https://www.dropbox.com/oauth2/authorize?client_id={}&response_type=code&token_access_type=offline",
        app_key
    );

    println!("Step 1: Visit this URL to authorize:");
    println!("\n  {}\n", auth_url);
    println!("Step 2: Click 'Allow' and copy the authorization code");
    print!("\nEnter authorization code: ");
    io::stdout().flush().unwrap();

    // Read authorization code from user
    let mut auth_code = String::new();
    io::stdin()
        .read_line(&mut auth_code)
        .map_err(|e| DbxUpError::Io(e))?;
    let auth_code = auth_code.trim();

    if auth_code.is_empty() {
        return Err(DbxUpError::Config("Authorization code cannot be empty".to_string()));
    }

    println!("\n🔄 Exchanging authorization code for tokens...");

    // Exchange code for tokens via Dropbox API
    let client = reqwest::Client::new();
    let params = [
        ("code", auth_code),
        ("grant_type", "authorization_code"),
        ("client_id", &app_key),
        ("client_secret", &app_secret),
    ];

    let response = client
        .post("https://api.dropboxapi.com/oauth2/token")
        .form(&params)
        .send()
        .await
        .map_err(|e| DbxUpError::Http(e))?;

    if !response.status().is_success() {
        let error_text = response.text().await.unwrap_or_else(|_| "Unknown error".to_string());
        return Err(DbxUpError::DropboxApi(format!(
            "Token exchange failed: {}",
            error_text
        )));
    }

    let token_response: serde_json::Value = response.json().await.map_err(|e| {
        DbxUpError::Config(format!("Failed to parse token response: {}", e))
    })?;

    // Extract refresh token
    let refresh_token = token_response["refresh_token"]
        .as_str()
        .ok_or_else(|| DbxUpError::Config("No refresh token in response".to_string()))?
        .to_string();

    println!("✓ Tokens received!");

    // Create authorization from refresh token
    let auth = Authorization::from_refresh_token(app_key, refresh_token);

    // Validate by making an API call
    use dropbox_sdk::{default_async_client::UserAuthDefaultClient, users};
    let client = UserAuthDefaultClient::new(auth.clone());

    let account = users::get_current_account(&client).await.map_err(|e| {
        DbxUpError::Authentication(format!("Failed to validate tokens: {}", e))
    })?;

    println!("✓ Authenticated as: {}\n", account.name.display_name);

    Ok(auth)
}

/// Get access token using refresh token (automatic, like Python code)
pub async fn get_access_token_from_refresh(
    app_key: &str,
    app_secret: &str,
    refresh_token: &str,
) -> Result<(String, i64)> {
    // This matches the Python code lines 163-171
    let client = reqwest::Client::new();

    // Basic auth with client_id:client_secret
    let auth_header = format!(
        "Basic {}",
        base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            format!("{}:{}", app_key, app_secret)
        )
    );

    let params = [
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
    ];

    let response = client
        .post("https://api.dropboxapi.com/oauth2/token")
        .header("Authorization", auth_header)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("Accept", "application/json")
        .form(&params)
        .send()
        .await
        .map_err(|e| DbxUpError::Http(e))?;

    if !response.status().is_success() {
        let error_text = response
            .text()
            .await
            .unwrap_or_else(|_| "Unknown error".to_string());
        return Err(DbxUpError::DropboxApi(format!(
            "Token refresh failed: {}",
            error_text
        )));
    }

    let token_response: serde_json::Value = response.json().await.map_err(|e| {
        DbxUpError::Config(format!("Failed to parse token response: {}", e))
    })?;

    let access_token = token_response["access_token"]
        .as_str()
        .ok_or_else(|| DbxUpError::Config("No access token in response".to_string()))?
        .to_string();

    let expires_in = token_response["expires_in"].as_i64().unwrap_or(14400);

    Ok((access_token, expires_in))
}

/// Delete cached token
pub fn clear_token() -> Result<()> {
    let cache_path = get_token_cache_path();

    if cache_path.exists() {
        fs::remove_file(&cache_path).map_err(|e| {
            DbxUpError::Io(io::Error::new(
                io::ErrorKind::Other,
                format!("Failed to delete token: {}", e),
            ))
        })?;
        println!("🗑️  Token cleared from: {}", cache_path.display());
    } else {
        println!("No saved token found.");
    }

    Ok(())
}
