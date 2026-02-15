/*
 *  upload_manager.rs
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

use crate::dropbox::client::DropboxClient;
use crate::dropbox::upload;
use crate::error::Result;
use crate::files::metadata::FileMetadata;
use std::sync::Arc;
use tokio::sync::Semaphore;

/// Manages concurrent file uploads with parallelism control
pub struct UploadManager {
    client: Arc<DropboxClient>,
    semaphore: Arc<Semaphore>,
    verbose: bool,
}

impl UploadManager {
    /// Create a new upload manager
    pub fn new(client: DropboxClient, max_parallel: usize, verbose: bool) -> Self {
        Self {
            client: Arc::new(client),
            semaphore: Arc::new(Semaphore::new(max_parallel)),
            verbose,
        }
    }

    /// Create upload manager from Arc<DropboxClient>
    pub fn new_from_arc(client: Arc<DropboxClient>, max_parallel: usize, verbose: bool) -> Self {
        Self {
            client,
            semaphore: Arc::new(Semaphore::new(max_parallel)),
            verbose,
        }
    }

    /// Upload files concurrently with controlled parallelism
    pub async fn upload_files(&self, files: Vec<FileMetadata>) -> Result<UploadStats> {
        let total_files = files.len();
        let mut tasks = Vec::new();

        for (idx, file) in files.into_iter().enumerate() {
            let client = Arc::clone(&self.client);
            let semaphore = Arc::clone(&self.semaphore);
            let verbose = self.verbose;

            // Spawn a task for each file
            let task = tokio::spawn(async move {
                // Acquire a permit from the semaphore
                let _permit = semaphore.acquire().await.unwrap();

                if verbose {
                    println!(
                        "[{}/{}] Uploading {} -> {}",
                        idx + 1,
                        total_files,
                        file.local_path.display(),
                        file.dropbox_path
                    );
                }

                // Upload the file
                let result = upload::upload_file(&client, &file).await;

                if !verbose {
                    if result.is_ok() {
                        println!("  ✓ {}", file.local_path.display());
                    }
                }

                // Return file path and result
                (file.local_path.to_string_lossy().to_string(), result)
            });

            tasks.push(task);
        }

        // Wait for all tasks to complete
        let mut stats = UploadStats::new();
        for task in tasks {
            match task.await {
                Ok((file_path, result)) => match result {
                    Ok(()) => {
                        stats.succeeded += 1;
                    }
                    Err(e) => {
                        stats.failed += 1;
                        stats
                            .errors
                            .push(format!("{}: {}", file_path, e));
                        eprintln!("  ✗ {}: {}", file_path, e);
                    }
                },
                Err(e) => {
                    stats.failed += 1;
                    stats
                        .errors
                        .push(format!("Task join error: {}", e));
                    eprintln!("  ✗ Task error: {}", e);
                }
            }
        }

        Ok(stats)
    }
}

/// Statistics about completed uploads
#[derive(Debug, Default)]
pub struct UploadStats {
    pub succeeded: usize,
    pub failed: usize,
    pub errors: Vec<String>,
}

impl UploadStats {
    fn new() -> Self {
        Self::default()
    }

    pub fn has_failures(&self) -> bool {
        self.failed > 0
    }
}
