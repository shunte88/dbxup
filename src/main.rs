use clap::Parser;
use std::path::PathBuf;

mod config;
mod dropbox;
mod error;
mod files;
mod oauth;
mod retry;
mod token_storage;
mod upload_manager;
mod watcher;

use config::{Config, UploadSource};
use error::{DbxUpError, Result};

#[derive(Parser, Debug)]
#[command(name = "dbxup")]
#[command(about = "Upload files to Dropbox with concurrent uploads", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Dropbox access token (or use DROPBOX_ACCESS_TOKEN env var)
    #[arg(short, long, env = "DROPBOX_ACCESS_TOKEN", global = true)]
    token: Option<String>,

    /// Dropbox app key for OAuth (or use DROPBOX_APP_KEY env var)
    #[arg(long, env = "DROPBOX_APP_KEY", global = true)]
    app_key: Option<String>,

    /// Dropbox app secret for OAuth (or use DROPBOX_APP_SECRET env var)
    #[arg(long, env = "DROPBOX_APP_SECRET", global = true)]
    app_secret: Option<String>,

    /// Use OAuth authorization flow
    #[arg(long, global = true)]
    oauth: bool,

    /// Upload a single file
    #[arg(short, long, conflicts_with = "dir")]
    file: Option<PathBuf>,

    /// Upload a directory (all files recursively)
    #[arg(short, long, conflicts_with = "file")]
    dir: Option<PathBuf>,

    /// Dropbox destination folder path
    #[arg(short = 'o', long, default_value = "/")]
    destination: String,

    /// Number of concurrent uploads
    #[arg(short, long, default_value_t = 5)]
    parallel: usize,

    /// Chunk size in MB for large file uploads
    #[arg(long, default_value_t = 8)]
    chunk_size: usize,

    /// Enable verbose logging
    #[arg(short, long)]
    verbose: bool,
}

#[derive(Parser, Debug)]
enum Commands {
    /// Perform OAuth authorization and save token
    Auth {
        /// Dropbox app key
        #[arg(long, env = "DROPBOX_APP_KEY")]
        app_key: String,
        /// Dropbox app secret
        #[arg(long, env = "DROPBOX_APP_SECRET")]
        app_secret: String,
    },
    /// Set up tokens with refresh token support (simpler method)
    Setup {
        /// Dropbox app key
        #[arg(long, env = "DROPBOX_APP_KEY")]
        app_key: String,
        /// Refresh token
        #[arg(long, env = "DROPBOX_REFRESH_TOKEN")]
        refresh_token: String,
    },
    /// Watch folders and auto-upload new files (daemon mode)
    Watch {
        /// Config file with folders to watch (JSON format)
        #[arg(short, long)]
        config: Option<PathBuf>,
        /// Folders to watch (format: /local/path:/dropbox/path)
        #[arg(short, long, num_args = 0..)]
        folders: Vec<String>,
        /// Number of concurrent uploads
        #[arg(short, long)]
        parallel: Option<usize>,
        /// Debounce delay in milliseconds
        #[arg(long)]
        debounce: Option<u64>,
    },
    /// Clear saved OAuth token
    Logout,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Handle subcommands first
    if let Some(command) = cli.command {
        match command {
            Commands::Auth { app_key, app_secret } => {
                // Use simple CLI OAuth flow
                println!("\n🔐 Dropbox Authorization\n");

                let auth_url = format!(
                    "https://www.dropbox.com/oauth2/authorize?client_id={}&response_type=code&token_access_type=offline",
                    app_key
                );

                println!("Step 1: Visit this URL to authorize:");
                println!("\n  {}\n", auth_url);
                println!("Step 2: Click 'Allow' and copy the authorization code");
                print!("\nEnter authorization code: ");
                use std::io::{self, Write};
                io::stdout().flush().unwrap();

                let mut auth_code = String::new();
                io::stdin().read_line(&mut auth_code).map_err(|e| DbxUpError::Io(e))?;
                let auth_code = auth_code.trim();

                if auth_code.is_empty() {
                    return Err(DbxUpError::Config("Authorization code cannot be empty".to_string()));
                }

                println!("\n🔄 Exchanging code for tokens...");

                // Exchange for refresh token
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

                let token_response: serde_json::Value = response.json().await.map_err(|e| {
                    DbxUpError::Config(format!("Failed to parse response: {}", e))
                })?;

                let refresh_token = token_response["refresh_token"]
                    .as_str()
                    .ok_or_else(|| DbxUpError::Config("No refresh token received".to_string()))?
                    .to_string();

                // Save tokens
                let mut token_store = token_storage::TokenStore::new(app_key, refresh_token);
                token_store.access_token = token_response["access_token"].as_str().map(String::from);
                token_store.expires_in = token_response["expires_in"].as_i64();
                token_store.save()?;

                println!("✓ Authorization complete!");
                println!("   Tokens saved. Future uploads will work automatically.\n");
                return Ok(());
            }
            Commands::Setup { app_key, refresh_token } => {
                let token_store = token_storage::TokenStore::new(app_key, refresh_token);
                token_store.save()?;
                println!("✓ Setup complete! Token will auto-refresh when needed.\n");
                return Ok(());
            }
            Commands::Watch { config, folders, parallel, debounce } => {
                // Initialize logging
                env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
                    .init();

                // Parse configuration
                let (watch_folders, parallel_uploads, debounce_ms) = if let Some(ref config_path) = config {
                    // Load from config file
                    let config_data = std::fs::read_to_string(&config_path)
                        .map_err(|e| DbxUpError::Config(format!("Failed to read config: {}", e)))?;
                    let config: serde_json::Value = serde_json::from_str(&config_data)
                        .map_err(|e| DbxUpError::Config(format!("Invalid config JSON: {}", e)))?;

                    let mut folders_vec = Vec::new();
                    if let Some(folders_array) = config["folders"].as_array() {
                        for folder in folders_array {
                            let local = folder["local_path"].as_str()
                                .ok_or_else(|| DbxUpError::Config("Missing local_path in config".to_string()))?;
                            let dropbox = folder["dropbox_path"].as_str()
                                .ok_or_else(|| DbxUpError::Config("Missing dropbox_path in config".to_string()))?;

                            let local_path = PathBuf::from(local);
                            if !local_path.exists() {
                                log::warn!("Folder does not exist: {}. Will monitor when it appears.", local_path.display());
                            }

                            folders_vec.push(watcher::WatchFolder {
                                local_path,
                                dropbox_path: dropbox.to_string(),
                            });
                        }
                    }

                    let parallel_cfg = parallel.or_else(|| config["settings"]["parallel_uploads"].as_u64().map(|v| v as usize))
                        .unwrap_or(5);
                    let debounce_cfg = debounce.or_else(|| config["settings"]["debounce_ms"].as_u64())
                        .unwrap_or(2000);

                    (folders_vec, parallel_cfg, debounce_cfg)
                } else {
                    // Parse from command line
                    let mut folders_vec = Vec::new();
                    for folder_spec in folders {
                        let parts: Vec<&str> = folder_spec.split(':').collect();
                        if parts.len() != 2 {
                            eprintln!("Invalid folder spec: {}. Use format: /local/path:/dropbox/path", folder_spec);
                            std::process::exit(1);
                        }

                        let local_path = PathBuf::from(parts[0]);
                        if !local_path.exists() {
                            log::warn!("Folder does not exist: {}. Will monitor when it appears.", local_path.display());
                        }

                        folders_vec.push(watcher::WatchFolder {
                            local_path,
                            dropbox_path: parts[1].to_string(),
                        });
                    }

                    (folders_vec, parallel.unwrap_or(5), debounce.unwrap_or(2000))
                };

                if watch_folders.is_empty() {
                    eprintln!("Error: No folders specified. Use --config or --folders");
                    std::process::exit(1);
                }

                // Get app key/secret from env
                let app_key = std::env::var("DROPBOX_APP_KEY")
                    .map_err(|_| DbxUpError::Config("DROPBOX_APP_KEY env var required".to_string()))?;
                let app_secret = std::env::var("DROPBOX_APP_SECRET")
                    .map_err(|_| DbxUpError::Config("DROPBOX_APP_SECRET env var required".to_string()))?;

                // Load tokens and get client
                let mut token_store = token_storage::TokenStore::load()?;
                let (access_token, expires_in) = oauth::get_access_token_from_refresh(
                    &app_key,
                    &app_secret,
                    &token_store.refresh_token,
                )
                .await?;

                token_store.access_token = Some(access_token.clone());
                token_store.expires_in = Some(expires_in);
                token_store.save()?;

                let auth = dropbox_sdk::oauth2::Authorization::from_long_lived_access_token(access_token);
                let client = dropbox::client::DropboxClient::from_authorization(auth);

                // Validate connection
                client.validate_token().await?;
                log::info!("Dropbox connection validated");

                // Start watcher
                let watch_config = watcher::WatchConfig {
                    folders: watch_folders,
                    parallel: parallel_uploads,
                    debounce_ms,
                };

                let mut folder_watcher = if let Some(cfg_path) = config {
                    watcher::FolderWatcher::new_with_config_file(watch_config, cfg_path, client)
                } else {
                    watcher::FolderWatcher::new(watch_config, client)
                };
                folder_watcher.watch().await?;

                return Ok(());
            }
            Commands::Logout => {
                token_storage::TokenStore::clear()?;
                return Ok(());
            }
        }
    }

    // Determine upload source
    let source = match (cli.file, cli.dir) {
        (Some(file), None) => UploadSource::File(file),
        (None, Some(dir)) => UploadSource::Directory(dir),
        (None, None) => {
            eprintln!("Error: Must specify either --file or --dir");
            std::process::exit(1);
        }
        (Some(_), Some(_)) => {
            // This shouldn't happen due to clap conflicts_with, but handle it anyway
            eprintln!("Error: Cannot specify both --file and --dir");
            std::process::exit(1);
        }
    };

    // Get authorization - automatic like Python code
    let auth = if let Some(token) = cli.token {
        // Direct token provided
        dropbox_sdk::oauth2::Authorization::from_long_lived_access_token(token)
    } else {
        // Automatic: try saved tokens first, then env vars
        let app_key = cli.app_key
            .or_else(|| std::env::var("DROPBOX_APP_KEY").ok())
            .ok_or_else(|| DbxUpError::Config(
                "App key required. Use --app-key or DROPBOX_APP_KEY env var, or run 'dbxup auth'".to_string()
            ))?;

        let app_secret = cli.app_secret
            .or_else(|| std::env::var("DROPBOX_APP_SECRET").ok())
            .ok_or_else(|| DbxUpError::Config(
                "App secret required. Use --app-secret or DROPBOX_APP_SECRET env var".to_string()
            ))?;

        // Try to load saved tokens
        match token_storage::TokenStore::load() {
            Ok(mut token_store) => {
                if cli.verbose {
                    println!("🔄 Using saved refresh token...");
                }

                // Get fresh access token using refresh token (like Python code)
                let (access_token, expires_in) = oauth::get_access_token_from_refresh(
                    &app_key,
                    &app_secret,
                    &token_store.refresh_token,
                )
                .await?;

                if cli.verbose {
                    println!("✓ Access token refreshed (expires in {}s)", expires_in);
                }

                // Update and save
                token_store.access_token = Some(access_token.clone());
                token_store.expires_in = Some(expires_in);
                token_store.save()?;

                // Create authorization from access token
                dropbox_sdk::oauth2::Authorization::from_long_lived_access_token(access_token)
            }
            Err(_) => {
                return Err(DbxUpError::Config(
                    "No saved tokens found. Run 'dbxup auth' first to set up.".to_string()
                ));
            }
        }
    };

    // Build and validate configuration
    let config = Config {
        token: String::new(), // No longer needed, using auth directly
        source,
        destination: cli.destination,
        parallel: cli.parallel,
        chunk_size_mb: cli.chunk_size,
        verbose: cli.verbose,
    }
    .validate()?;

    if config.verbose {
        println!("Configuration:");
        println!("  Source: {:?}", config.source);
        println!("  Destination: {}", config.destination);
        println!("  Parallel uploads: {}", config.parallel);
        println!("  Chunk size: {}MB", config.chunk_size_mb);
    }

    // Phase 2: Discover files to upload
    let files = discover_files(&config)?;

    if config.verbose {
        println!("\nDiscovered {} files:", files.len());
        for file in &files {
            let size_mb = file.size as f64 / (1024.0 * 1024.0);
            let upload_type = if file.requires_chunked_upload() {
                "chunked"
            } else {
                "simple"
            };
            println!(
                "  {} -> {} ({:.2}MB, {})",
                file.local_path.display(),
                file.dropbox_path,
                size_mb,
                upload_type
            );
        }
    } else {
        println!("Found {} files to upload", files.len());
    }

    // Phase 3: Initialize Dropbox client and validate token
    if config.verbose {
        println!("\nValidating Dropbox credentials...");
    }
    let client = dropbox::client::DropboxClient::from_authorization(auth);
    client.validate_token().await?;
    if config.verbose {
        println!("Credentials validated successfully");
    }

    // Phase 4: Upload files concurrently
    println!("\nUploading {} files (parallelism: {})...", files.len(), config.parallel);

    let manager = upload_manager::UploadManager::new(client, config.parallel, config.verbose);
    let stats = manager.upload_files(files).await?;

    // Print summary
    println!("\n✓ Successfully uploaded {} files!", stats.succeeded);
    if stats.has_failures() {
        eprintln!("✗ Failed to upload {} files", stats.failed);
        eprintln!("\nErrors:");
        for error in &stats.errors {
            eprintln!("  {}", error);
        }
        std::process::exit(1);
    }

    Ok(())
}

/// Discover files to upload based on configuration
fn discover_files(config: &Config) -> Result<Vec<files::metadata::FileMetadata>> {
    use files::scanner::FileScanner;

    let (base_path, is_dir) = match &config.source {
        UploadSource::File(path) => (path.clone(), false),
        UploadSource::Directory(path) => (path.clone(), true),
    };

    let scanner = FileScanner::new(base_path.clone(), config.destination.clone());

    if is_dir {
        scanner.scan_directory(&base_path)
    } else {
        scanner.scan_file(&base_path)
    }
}
