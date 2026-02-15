use crate::dropbox::client::DropboxClient;
use crate::error::{DbxUpError, Result};
use crate::files::metadata::FileMetadata;
use crate::upload_manager::UploadManager;
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::sleep;
use serde_json;

/// Configuration for folder watching
#[derive(Debug, Clone)]
pub struct WatchConfig {
    pub folders: Vec<WatchFolder>,
    pub parallel: usize,
    pub debounce_ms: u64,
}

#[derive(Debug, Clone)]
pub struct WatchFolder {
    pub local_path: PathBuf,
    pub dropbox_path: String,
}

/// Folder watcher daemon
pub struct FolderWatcher {
    config: WatchConfig,
    config_file: Option<PathBuf>,
    client: Arc<DropboxClient>,
}

impl FolderWatcher {
    pub fn new(config: WatchConfig, client: DropboxClient) -> Self {
        Self {
            config,
            config_file: None,
            client: Arc::new(client),
        }
    }

    pub fn new_with_config_file(config: WatchConfig, config_file: PathBuf, client: DropboxClient) -> Self {
        Self {
            config,
            config_file: Some(config_file),
            client: Arc::new(client),
        }
    }

    /// Start watching folders and uploading new files
    pub async fn watch(&mut self) -> Result<()> {
        log::info!("Starting folder watcher for {} folders", self.config.folders.len());

        for folder in &self.config.folders {
            log::info!("  Watching: {} -> {}", folder.local_path.display(), folder.dropbox_path);
        }

        // Create channel for file events
        let (tx, mut rx) = mpsc::unbounded_channel();

        // Set up file system watcher
        let mut watcher = RecommendedWatcher::new(
            move |res: notify::Result<Event>| {
                if let Ok(event) = res {
                    let _ = tx.send(event);
                }
            },
            Config::default().with_poll_interval(Duration::from_secs(2)),
        )
        .map_err(|e| DbxUpError::Config(format!("Failed to create watcher: {}", e)))?;

        // Watch all configured folders (with error handling for missing folders)
        let mut watching = Vec::new();
        for folder in &self.config.folders {
            match watcher.watch(&folder.local_path, RecursiveMode::Recursive) {
                Ok(_) => {
                    log::info!("✓ Watching: {}", folder.local_path.display());
                    watching.push(folder.local_path.clone());
                }
                Err(e) => {
                    log::warn!("⚠ Cannot watch {} ({}). Will retry...", folder.local_path.display(), e);
                }
            }
        }

        if watching.is_empty() {
            log::warn!("No folders are currently being watched. Waiting for folders to appear...");
        }

        // Watch config file if specified
        if let Some(config_path) = &self.config_file {
            if let Err(e) = watcher.watch(config_path, RecursiveMode::NonRecursive) {
                log::warn!("Cannot watch config file {}: {}", config_path.display(), e);
            } else {
                log::info!("Watching config file for changes: {}", config_path.display());
            }
        }

        log::info!("Watcher started. Monitoring for new files...");
        println!("👁️  Watching {} folder(s). Press Ctrl+C to stop.\n", watching.len());
        if self.config_file.is_some() {
            println!("   Config file changes will be detected and reloaded automatically.\n");
        }

        // Track recent uploads to avoid duplicates (debouncing)
        let mut pending_uploads: HashMap<PathBuf, tokio::time::Instant> = HashMap::new();

        // Track last folder recheck time
        let mut last_folder_check = tokio::time::Instant::now();
        let folder_check_interval = Duration::from_secs(30);

        // Process events
        loop {
            tokio::select! {
                Some(event) = rx.recv() => {
                    // Check if config file was modified
                    if let Some(config_path) = &self.config_file {
                        if event.paths.iter().any(|p| p == config_path) {
                            if matches!(event.kind, EventKind::Modify(_)) {
                                log::info!("Config file changed, reloading...");
                                match self.reload_config_from_file(&mut watcher).await {
                                    Ok(new_watching) => {
                                        watching = new_watching;
                                        pending_uploads.clear();
                                        log::info!("✓ Config reloaded successfully. Now watching {} folder(s)", watching.len());
                                        println!("\n🔄 Config reloaded! Now watching {} folder(s)\n", watching.len());
                                    }
                                    Err(e) => {
                                        log::error!("Failed to reload config: {}", e);
                                        eprintln!("\n✗ Config reload failed: {}\n", e);
                                    }
                                }
                                continue;
                            }
                        }
                    }

                    // Handle folder deletion/errors
                    if let EventKind::Remove(_) = event.kind {
                        for path in &event.paths {
                            if path.is_dir() {
                                log::warn!("Folder removed or unmounted: {}", path.display());
                            }
                        }
                    }
                    self.handle_event(event, &mut pending_uploads).await;
                }
                _ = sleep(Duration::from_millis(self.config.debounce_ms)) => {
                    // Process pending uploads after debounce period
                    self.process_pending_uploads(&mut pending_uploads).await;

                    // Periodically recheck folders (for ones that were missing)
                    let now = tokio::time::Instant::now();
                    if now.duration_since(last_folder_check) >= folder_check_interval {
                        self.recheck_folders(&mut watcher).await;
                        last_folder_check = now;
                    }
                }
            }
        }
    }

    async fn handle_event(&self, event: Event, pending: &mut HashMap<PathBuf, tokio::time::Instant>) {
        // Only handle create and modify events
        if !matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_)) {
            return;
        }

        for path in event.paths {
            // Skip if not a file
            if !path.is_file() {
                continue;
            }

            // Skip hidden files
            if let Some(filename) = path.file_name() {
                if filename.to_string_lossy().starts_with('.') {
                    continue;
                }
            }

            // Add to pending uploads (with debounce)
            pending.insert(path, tokio::time::Instant::now());
        }
    }

    async fn process_pending_uploads(&self, pending: &mut HashMap<PathBuf, tokio::time::Instant>) {
        let now = tokio::time::Instant::now();
        let debounce_duration = Duration::from_millis(self.config.debounce_ms);

        // Find files ready to upload (debounce period passed)
        let ready: Vec<PathBuf> = pending
            .iter()
            .filter(|(_, instant)| now.duration_since(**instant) >= debounce_duration)
            .map(|(path, _)| path.clone())
            .collect();

        if ready.is_empty() {
            return;
        }

        // Prepare files for upload
        let mut files_to_upload = Vec::new();

        for local_path in &ready {
            // Find which watch folder this file belongs to
            if let Some(watch_folder) = self.find_watch_folder(local_path) {
                // Calculate relative path and Dropbox destination
                match self.create_file_metadata(local_path, watch_folder) {
                    Ok(metadata) => {
                        files_to_upload.push(metadata);
                        pending.remove(local_path);
                    }
                    Err(e) => {
                        log::error!("Failed to prepare {}: {}", local_path.display(), e);
                        pending.remove(local_path);
                    }
                }
            }
        }

        // Upload files
        if !files_to_upload.is_empty() {
            self.upload_files(files_to_upload).await;
        }
    }

    fn find_watch_folder(&self, path: &Path) -> Option<&WatchFolder> {
        self.config
            .folders
            .iter()
            .find(|folder| path.starts_with(&folder.local_path))
    }

    fn create_file_metadata(&self, local_path: &Path, watch_folder: &WatchFolder) -> Result<FileMetadata> {
        // Get file size
        let metadata = std::fs::metadata(local_path)?;
        let size = metadata.len();

        // Check Dropbox size limit
        if size > 350 * 1024 * 1024 * 1024 {
            return Err(DbxUpError::FileTooLarge(local_path.to_path_buf()));
        }

        // Calculate relative path
        let relative = local_path
            .strip_prefix(&watch_folder.local_path)
            .map_err(|_| DbxUpError::InvalidPath("Path mismatch".to_string()))?;

        // Build Dropbox path
        let relative_str = relative.to_string_lossy().replace('\\', "/");
        let dropbox_path = if watch_folder.dropbox_path.ends_with('/') {
            format!("{}{}", watch_folder.dropbox_path, relative_str)
        } else {
            format!("{}/{}", watch_folder.dropbox_path, relative_str)
        };

        Ok(FileMetadata::new(
            local_path.to_path_buf(),
            dropbox_path,
            size,
        ))
    }

    async fn recheck_folders(&self, watcher: &mut RecommendedWatcher) {
        // Try to watch folders that weren't being watched
        for folder in &self.config.folders {
            if folder.local_path.exists() {
                // Try to watch (might already be watched, that's ok)
                if let Err(e) = watcher.watch(&folder.local_path, RecursiveMode::Recursive) {
                    // Only log if it's not "already watching" error
                    if !e.to_string().contains("already") {
                        log::debug!("Cannot watch {}: {}", folder.local_path.display(), e);
                    }
                }
            }
        }
    }

    async fn reload_config_from_file(&mut self, watcher: &mut RecommendedWatcher) -> Result<Vec<PathBuf>> {
        let config_path = self.config_file.as_ref()
            .ok_or_else(|| DbxUpError::Config("No config file specified".to_string()))?;

        // Read and parse config file
        let config_data = std::fs::read_to_string(config_path)
            .map_err(|e| DbxUpError::Config(format!("Failed to read config: {}", e)))?;
        let config: serde_json::Value = serde_json::from_str(&config_data)
            .map_err(|e| DbxUpError::Config(format!("Invalid config JSON: {}", e)))?;

        // Parse folders
        let mut folders_vec = Vec::new();
        if let Some(folders_array) = config["folders"].as_array() {
            for folder in folders_array {
                let local = folder["local_path"].as_str()
                    .ok_or_else(|| DbxUpError::Config("Missing local_path in config".to_string()))?;
                let dropbox = folder["dropbox_path"].as_str()
                    .ok_or_else(|| DbxUpError::Config("Missing dropbox_path in config".to_string()))?;

                folders_vec.push(WatchFolder {
                    local_path: PathBuf::from(local),
                    dropbox_path: dropbox.to_string(),
                });
            }
        }

        // Parse settings
        let parallel = config["settings"]["parallel_uploads"].as_u64()
            .map(|v| v as usize)
            .unwrap_or(self.config.parallel);
        let debounce_ms = config["settings"]["debounce_ms"].as_u64()
            .unwrap_or(self.config.debounce_ms);

        // Update config
        self.config = WatchConfig {
            folders: folders_vec,
            parallel,
            debounce_ms,
        };

        // Re-watch all folders
        let mut watching = Vec::new();
        for folder in &self.config.folders {
            match watcher.watch(&folder.local_path, RecursiveMode::Recursive) {
                Ok(_) => {
                    log::info!("✓ Watching: {}", folder.local_path.display());
                    watching.push(folder.local_path.clone());
                }
                Err(e) => {
                    log::warn!("⚠ Cannot watch {} ({}). Will retry...", folder.local_path.display(), e);
                }
            }
        }

        Ok(watching)
    }

    async fn upload_files(&self, files: Vec<FileMetadata>) {
        let count = files.len();
        log::info!("Uploading {} new file(s)", count);

        println!("📤 Uploading {} new file(s)...", count);

        let manager = UploadManager::new_from_arc(
            Arc::clone(&self.client),
            self.config.parallel,
            false, // non-verbose for daemon mode
        );

        match manager.upload_files(files).await {
            Ok(stats) => {
                if stats.succeeded > 0 {
                    log::info!("✓ Uploaded {} file(s)", stats.succeeded);
                    println!("   ✓ Uploaded {} file(s)", stats.succeeded);
                }
                if stats.failed > 0 {
                    log::error!("✗ Failed to upload {} file(s)", stats.failed);
                    println!("   ✗ Failed: {} file(s)", stats.failed);
                    for error in &stats.errors {
                        log::error!("     {}", error);
                    }
                }
            }
            Err(e) => {
                log::error!("Upload error: {}", e);
                eprintln!("   ✗ Upload error: {}", e);
            }
        }
    }
}
