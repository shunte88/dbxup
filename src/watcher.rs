/*
 *  watcher.rs
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

use crate::dropbox::client::DropboxClient;
use crate::error::{DbxUpError, Result};
use crate::files::metadata::FileMetadata;
use crate::persistent_queue::{PersistentQueue, QueueEntry};
use crate::upload_manager::UploadManager;
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::{mpsc, RwLock};
use tokio::time::{sleep, interval};
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

/// Track file stability for detecting when files are done being written
#[derive(Debug, Clone)]
struct FileStabilityCheck {
    size: u64,
    last_modified: SystemTime,
    first_seen: tokio::time::Instant,
}

/// Shared state for token refresh and queue management
struct WatcherState {
    client: Arc<DropboxClient>,
    uploading: HashSet<PathBuf>,
    queue: PersistentQueue,
}

/// Folder watcher daemon with background upload queue
pub struct FolderWatcher {
    config: WatchConfig,
    config_file: Option<PathBuf>,
    state: Arc<RwLock<WatcherState>>,
    app_key: String,
    app_secret: String,
}

impl FolderWatcher {
    pub fn new(config: WatchConfig, client: DropboxClient, app_key: String, app_secret: String) -> Result<Self> {
        let queue = PersistentQueue::new()?;

        let state = WatcherState {
            client: Arc::new(client),
            uploading: HashSet::new(),
            queue,
        };

        Ok(Self {
            config,
            config_file: None,
            state: Arc::new(RwLock::new(state)),
            app_key,
            app_secret,
        })
    }

    pub fn new_with_config_file(
        config: WatchConfig,
        config_file: PathBuf,
        client: DropboxClient,
        app_key: String,
        app_secret: String,
    ) -> Result<Self> {
        let queue = PersistentQueue::new()?;

        let state = WatcherState {
            client: Arc::new(client),
            uploading: HashSet::new(),
            queue,
        };

        Ok(Self {
            config,
            config_file: Some(config_file),
            state: Arc::new(RwLock::new(state)),
            app_key,
            app_secret,
        })
    }

    /// Start watching folders and uploading new files
    pub async fn watch(&mut self) -> Result<()> {
        log::info!("Starting folder watcher for {} folders", self.config.folders.len());

        // Log persisted queue status
        {
            let state = self.state.read().await;
            let queue_len = state.queue.len();
            if queue_len > 0 {
                log::info!("Loaded {} entries from persistent queue", queue_len);
                let resumable = state.queue.get_resumable_entries().len();
                if resumable > 0 {
                    log::info!("  {} entries have resumable upload sessions", resumable);
                }
            }
        }

        for folder in &self.config.folders {
            log::info!("  Watching: {} -> {}", folder.local_path.display(), folder.dropbox_path);
        }

        // Create channel for file system events only
        let (event_tx, mut event_rx) = mpsc::unbounded_channel();

        // Set up file system watcher
        let mut watcher = RecommendedWatcher::new(
            move |res: notify::Result<Event>| {
                if let Ok(event) = res {
                    let _ = event_tx.send(event);
                }
            },
            Config::default().with_poll_interval(Duration::from_secs(2)),
        )
        .map_err(|e| DbxUpError::Config(format!("Failed to create watcher: {}", e)))?;

        // Watch all configured folders
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

        // Start background upload processor (processes from persistent queue)
        let _upload_task = {
            let state = Arc::clone(&self.state);
            let parallel = self.config.parallel;
            tokio::spawn(async move {
                Self::upload_processor(state, parallel).await;
            })
        };

        // Start periodic queue saver
        let _queue_saver_task = {
            let state = Arc::clone(&self.state);
            tokio::spawn(async move {
                Self::queue_saver_loop(state).await;
            })
        };

        // Start token refresh task
        let _token_refresh_task = {
            let state = Arc::clone(&self.state);
            let app_key = self.app_key.clone();
            let app_secret = self.app_secret.clone();
            tokio::spawn(async move {
                Self::token_refresh_loop(state, app_key, app_secret).await;
            })
        };

        // Perform initial scan of existing files (merges with persisted queue)
        log::info!("Scanning existing files in watched folders...");
        self.scan_existing_files().await;

        // Track file stability
        let mut file_stability: HashMap<PathBuf, FileStabilityCheck> = HashMap::new();

        // Track last folder recheck time
        let mut last_folder_check = tokio::time::Instant::now();
        let folder_check_interval = Duration::from_secs(30);

        // Main event loop
        loop {
            tokio::select! {
                Some(event) = event_rx.recv() => {
                    // Check if config file was modified
                    if let Some(config_path) = &self.config_file {
                        if event.paths.iter().any(|p| p == config_path) {
                            if matches!(event.kind, EventKind::Modify(_)) {
                                log::info!("Config file changed, reloading...");
                                match self.reload_config_from_file(&mut watcher).await {
                                    Ok(new_watching) => {
                                        watching = new_watching;
                                        file_stability.clear();
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

                    // Handle folder deletion
                    if let EventKind::Remove(_) = event.kind {
                        for path in &event.paths {
                            if path.is_dir() {
                                log::warn!("Folder removed or unmounted: {}", path.display());
                            }
                        }
                    }

                    // Handle file events
                    self.handle_file_event(event, &mut file_stability).await;
                }
                _ = sleep(Duration::from_millis(500)) => {
                    // Periodically check file stability and queue uploads
                    self.check_stable_files(&mut file_stability).await;

                    // Periodically recheck folders
                    let now = tokio::time::Instant::now();
                    if now.duration_since(last_folder_check) >= folder_check_interval {
                        self.recheck_folders(&mut watcher).await;
                        last_folder_check = now;
                    }

                    // Log queue stats periodically
                    if now.duration_since(last_folder_check).as_secs() % 60 == 0 {
                        let state = self.state.read().await;
                        if !file_stability.is_empty() || !state.uploading.is_empty() {
                            log::info!(
                                "Queue status: {} pending, {} uploading",
                                file_stability.len(),
                                state.uploading.len()
                            );
                        }
                    }
                }
            }
        }
    }

    /// Background task: Token refresh loop
    async fn token_refresh_loop(
        state: Arc<RwLock<WatcherState>>,
        app_key: String,
        app_secret: String,
    ) {
        let mut refresh_interval = interval(Duration::from_secs(3600)); // Refresh every hour

        loop {
            refresh_interval.tick().await;

            log::info!("🔄 Refreshing Dropbox access token...");

            // Load tokens and refresh
            match crate::token_storage::TokenStore::load() {
                Ok(mut token_store) => {
                    match crate::oauth::get_access_token_from_refresh(
                        &app_key,
                        &app_secret,
                        &token_store.refresh_token,
                    )
                    .await
                    {
                        Ok((access_token, expires_in)) => {
                            // Update stored tokens
                            token_store.access_token = Some(access_token.clone());
                            token_store.expires_in = Some(expires_in);
                            if let Err(e) = token_store.save() {
                                log::error!("Failed to save refreshed token: {}", e);
                            }

                            // Create new client with fresh token
                            let auth = dropbox_sdk::oauth2::Authorization::from_long_lived_access_token(access_token);
                            let new_client = DropboxClient::from_authorization(auth);

                            // Update shared state
                            let mut state_guard = state.write().await;
                            state_guard.client = Arc::new(new_client);

                            log::info!("✓ Token refreshed successfully (expires in {}s)", expires_in);
                        }
                        Err(e) => {
                            log::error!("Failed to refresh token: {}", e);
                        }
                    }
                }
                Err(e) => {
                    log::error!("Failed to load token store: {}", e);
                }
            }
        }
    }

    /// Background task: Periodic queue saver
    async fn queue_saver_loop(state: Arc<RwLock<WatcherState>>) {
        let mut save_interval = interval(Duration::from_secs(30)); // Save every 30 seconds

        loop {
            save_interval.tick().await;

            let state_guard = state.read().await;
            if let Err(e) = state_guard.queue.save() {
                log::error!("Failed to save queue: {}", e);
            }
        }
    }

    /// Background task: Upload processor (processes persistent queue)
    async fn upload_processor(
        state: Arc<RwLock<WatcherState>>,
        parallel: usize,
    ) {
        log::info!("Upload processor started (parallelism: {})", parallel);
        let mut process_interval = interval(Duration::from_secs(1)); // Check queue every second

        loop {
            process_interval.tick().await;

            // Get entries ready for upload
            let entries_to_process = {
                let state_guard = state.read().await;
                let ready = state_guard.queue.get_ready_entries();

                // Filter out ones already uploading
                ready
                    .into_iter()
                    .filter(|e| !state_guard.uploading.contains(&e.local_path))
                    .take(parallel) // Limit to parallelism
                    .map(|e| e.clone())
                    .collect::<Vec<_>>()
            };

            if entries_to_process.is_empty() {
                continue;
            }

            // Process each entry
            for entry in entries_to_process {
                let state_clone = Arc::clone(&state);
                let path = entry.local_path.clone();

                // Mark as uploading
                {
                    let mut state_guard = state.write().await;
                    state_guard.uploading.insert(path.clone());
                }

                // Spawn upload task
                tokio::spawn(async move {
                    Self::process_upload(state_clone, entry).await;
                });
            }
        }
    }

    /// Process a single upload from the queue
    async fn process_upload(state: Arc<RwLock<WatcherState>>, entry: QueueEntry) {
        let path = entry.local_path.clone();
        let dropbox_path = entry.dropbox_path.clone();
        let size = entry.size;

        log::info!(
            "📤 Starting upload: {} -> {} ({} bytes)",
            path.display(),
            dropbox_path,
            size
        );

        // Get current client
        let client = {
            let state_guard = state.read().await;
            Arc::clone(&state_guard.client)
        };

        // Create file metadata
        let metadata = FileMetadata::new(path.clone(), dropbox_path.clone(), size);

        // Upload the file
        let manager = UploadManager::new_from_arc(client, 1, false);
        let result = manager.upload_files(vec![metadata]).await;

        match result {
            Ok(stats) => {
                if stats.succeeded > 0 {
                    log::info!("✓ Upload complete: {}", path.display());
                    println!("   ✓ Uploaded: {}", path.display());

                    // Remove from queue on success
                    let mut state_guard = state.write().await;
                    state_guard.queue.remove(&path);
                    state_guard.uploading.remove(&path);
                } else if stats.failed > 0 {
                    log::error!("✗ Upload failed: {}", path.display());
                    println!("   ✗ Failed: {}", path.display());

                    // Increment retry count
                    let mut state_guard = state.write().await;
                    state_guard.queue.increment_retry(&path);
                    state_guard.uploading.remove(&path);

                    // Log errors
                    for error in &stats.errors {
                        log::error!("     {}", error);
                    }
                }
            }
            Err(e) => {
                log::error!("Upload error for {}: {}", path.display(), e);
                eprintln!("   ✗ Upload error: {}: {}", path.display(), e);

                // Increment retry count
                let mut state_guard = state.write().await;
                state_guard.queue.increment_retry(&path);
                state_guard.uploading.remove(&path);
            }
        }
    }

    /// Handle file system events and track file stability
    async fn handle_file_event(
        &self,
        event: Event,
        stability: &mut HashMap<PathBuf, FileStabilityCheck>,
    ) {
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

            // Check if already uploading
            {
                let state = self.state.read().await;
                if state.uploading.contains(&path) {
                    log::debug!("Skipping {} (already uploading)", path.display());
                    continue;
                }
            }

            // Get file metadata
            match std::fs::metadata(&path) {
                Ok(meta) => {
                    let size = meta.len();
                    let modified = meta.modified().unwrap_or(SystemTime::now());

                    // Update stability tracking
                    if let Some(existing) = stability.get_mut(&path) {
                        // File changed - reset stability
                        if existing.size != size || existing.last_modified != modified {
                            log::debug!("File modified: {} (size: {} -> {})", path.display(), existing.size, size);
                            existing.size = size;
                            existing.last_modified = modified;
                            existing.first_seen = tokio::time::Instant::now();
                        }
                    } else {
                        // New file - start tracking
                        log::debug!("New file detected: {} ({} bytes)", path.display(), size);
                        stability.insert(
                            path.clone(),
                            FileStabilityCheck {
                                size,
                                last_modified: modified,
                                first_seen: tokio::time::Instant::now(),
                            },
                        );
                    }
                }
                Err(e) => {
                    log::debug!("Failed to get metadata for {}: {}", path.display(), e);
                }
            }
        }
    }

    /// Scan existing files on startup and queue missing ones (merges with persisted queue)
    async fn scan_existing_files(&self) {
        use walkdir::WalkDir;

        let mut total_found = 0;
        let mut total_queued = 0;

        for watch_folder in &self.config.folders {
            if !watch_folder.local_path.exists() {
                log::warn!("Folder does not exist: {}", watch_folder.local_path.display());
                continue;
            }

            log::info!("Scanning: {}", watch_folder.local_path.display());

            for entry in WalkDir::new(&watch_folder.local_path)
                .follow_links(false)
                .into_iter()
                .filter_entry(|e| {
                    // Skip hidden files/directories
                    e.file_name()
                        .to_str()
                        .map(|s| !s.starts_with('.'))
                        .unwrap_or(false)
                })
            {
                match entry {
                    Ok(entry) => {
                        if !entry.file_type().is_file() {
                            continue;
                        }

                        total_found += 1;

                        let local_path = entry.path();

                        // Create file metadata
                        match self.create_file_metadata(local_path, watch_folder) {
                            Ok(metadata) => {
                                // Check if file exists in Dropbox
                                let client = {
                                    let state = self.state.read().await;
                                    Arc::clone(&state.client)
                                };

                                // Check if already in queue (from persisted state)
                                let already_queued = {
                                    let state = self.state.read().await;
                                    state.queue.contains(&metadata.local_path)
                                };

                                if already_queued {
                                    log::debug!(
                                        "File already in queue (from persistence): {}",
                                        metadata.local_path.display()
                                    );
                                    continue;
                                }
                                let mut should_upload = false;
                                match client.get_file_size(&metadata.dropbox_path).await {
                                    Ok(remote_size) => {
                                        should_upload = match remote_size {
                                            None => {
                                                // File doesn't exist
                                                log::info!(
                                                    "File not in Dropbox, queueing: {} -> {}",
                                                    metadata.local_path.display(),
                                                    metadata.dropbox_path
                                                );
                                                true
                                            }
                                            Some(size) if size != metadata.size => {
                                                // File exists but different size
                                                log::info!(
                                                    "File size differs (local: {}, remote: {}), queueing: {}",
                                                    metadata.size,
                                                    size,
                                                    metadata.dropbox_path
                                                );
                                                true
                                            }
                                            Some(_) => {
                                                // File exists with same size - skip
                                                log::debug!(
                                                    "File already exists with same size, skipping: {}",
                                                    metadata.dropbox_path
                                                );
                                                false
                                            }
                                        };

                                    }
                                    Err(e) => {
                                        //log::error!(
                                        //    "Failed to check file metadata in Dropbox: {} - {}",
                                        //    metadata.dropbox_path,
                                        //    e
                                        //);
                                        // file does not exist - should be catching this!!!
                                        log::info!("Dropbox has no such file {}", e);
                                        should_upload = true;
                                    }
                                }
                                if should_upload {
                                    let entry = QueueEntry::new(
                                        metadata.local_path.clone(),
                                        metadata.dropbox_path.clone(),
                                        metadata.size,
                                    );
                                    let mut state = self.state.write().await;
                                    state.queue.add(entry);
                                    total_queued += 1;
                                }
                            }
                            Err(e) => {
                                log::error!("Failed to prepare {}: {}", local_path.display(), e);
                            }
                        }
                    }
                    Err(e) => {
                        log::error!("Error scanning directory: {}", e);
                    }
                }
            }
        }

        log::info!(
            "Startup scan complete: {} files found, {} queued for upload",
            total_found,
            total_queued
        );

        if total_queued > 0 {
            println!(
                "📋 Queued {} existing file(s) for upload (not in Dropbox)\n",
                total_queued
            );
        }
    }

    /// Check stable files and queue them for upload
    async fn check_stable_files(
        &self,
        stability: &mut HashMap<PathBuf, FileStabilityCheck>,
    ) {
        let now = tokio::time::Instant::now();
        let stability_threshold = Duration::from_millis(self.config.debounce_ms);

        let mut to_upload = Vec::new();
        let mut to_remove = Vec::new();

        for (path, check) in stability.iter() {
            // Check if file has been stable for long enough
            if now.duration_since(check.first_seen) >= stability_threshold {
                // Verify file still exists and hasn't changed
                match std::fs::metadata(path) {
                    Ok(meta) => {
                        let current_size = meta.len();
                        let current_modified = meta.modified().unwrap_or(SystemTime::now());

                        if current_size == check.size && current_modified == check.last_modified {
                            // File is stable - prepare for upload
                            if let Some(watch_folder) = self.find_watch_folder(path) {
                                match self.create_file_metadata(path, watch_folder) {
                                    Ok(metadata) => {
                                        // Check if file already exists in Dropbox
                                        let client = {
                                            let state = self.state.read().await;
                                            Arc::clone(&state.client)
                                        };

                                        match client.get_file_size(&metadata.dropbox_path).await {
                                            Ok(remote_size) => {
                                                let should_upload = match remote_size {
                                                    None => {
                                                        // File doesn't exist
                                                        log::info!(
                                                            "New file detected, will upload: {}",
                                                            metadata.dropbox_path
                                                        );
                                                        true
                                                    }
                                                    Some(size) if size != metadata.size => {
                                                        // File exists but different size - upload to replace
                                                        log::info!(
                                                            "File size changed (local: {}, remote: {}), will replace: {}",
                                                            metadata.size,
                                                            size,
                                                            metadata.dropbox_path
                                                        );
                                                        true
                                                    }
                                                    Some(_) => {
                                                        // File exists with same size - skip
                                                        log::info!(
                                                            "File already exists with same size, skipping: {}",
                                                            metadata.dropbox_path
                                                        );
                                                        false
                                                    }
                                                };

                                                if should_upload {
                                                    to_upload.push(metadata);
                                                }
                                            }
                                            Err(e) => {
                                                log::error!(
                                                    "Failed to check file metadata, queueing anyway: {} - {}",
                                                    metadata.dropbox_path,
                                                    e
                                                );
                                                // Queue anyway if we can't check
                                                to_upload.push(metadata);
                                            }
                                        }
                                        to_remove.push(path.clone());
                                    }
                                    Err(e) => {
                                        log::error!("Failed to prepare {}: {}", path.display(), e);
                                        to_remove.push(path.clone());
                                    }
                                }
                            }
                        } else {
                            log::debug!("File still changing: {}", path.display());
                        }
                    }
                    Err(e) => {
                        log::debug!("File disappeared: {} ({})", path.display(), e);
                        to_remove.push(path.clone());
                    }
                }
            }
        }

        // Queue uploads
        if !to_upload.is_empty() {
            let mut state = self.state.write().await;

            for metadata in to_upload {
                log::info!("File stable, queueing for upload: {}", metadata.local_path.display());

                let entry = QueueEntry::new(
                    metadata.local_path.clone(),
                    metadata.dropbox_path.clone(),
                    metadata.size,
                );

                state.queue.add(entry);
            }
        }

        // Remove processed files from stability tracking
        for path in to_remove {
            stability.remove(&path);
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

}
