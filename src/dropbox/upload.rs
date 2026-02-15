/*
 *  upload.rs
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
use crate::retry::{retry_with_backoff, RetryConfig};
use bytes::Bytes;
use dropbox_sdk::files;
use std::fs::File;
use std::io::Read;

/// Upload a file to Dropbox with retry logic
pub async fn upload_file(client: &DropboxClient, file: &FileMetadata) -> Result<()> {
    let retry_config = RetryConfig::default();

    retry_with_backoff(&retry_config, || async {
        if file.requires_chunked_upload() {
            upload_chunked(client, file).await
        } else {
            upload_simple(client, file).await
        }
    })
    .await
}

/// Simple upload for files < 150MB
async fn upload_simple(client: &DropboxClient, file: &FileMetadata) -> Result<()> {
    // Read file contents
    let mut f = File::open(&file.local_path)?;
    let mut contents = Vec::new();
    f.read_to_end(&mut contents)?;

    // Convert to Bytes
    let bytes = Bytes::from(contents);

    // Prepare upload parameters
    let upload_arg = files::UploadArg::new(file.dropbox_path.clone())
        .with_mode(files::WriteMode::Overwrite)
        .with_autorename(false)
        .with_mute(false);

    // Upload the file
    files::upload(client.client(), &upload_arg, bytes)
        .await
        .map_err(|e| DbxUpError::DropboxApi(format!("Upload failed: {}", e)))?;

    Ok(())
}

/// Chunked upload for files >= 150MB
async fn upload_chunked(client: &DropboxClient, file: &FileMetadata) -> Result<()> {
    use tokio::fs::File as TokioFile;
    use tokio::io::AsyncReadExt;

    // Default chunk size: 8MB
    const CHUNK_SIZE: usize = 8 * 1024 * 1024;

    // Open file for reading
    let mut f = TokioFile::open(&file.local_path).await?;

    // Read first chunk
    let mut buffer = vec![0u8; CHUNK_SIZE];
    let first_chunk_size = f.read(&mut buffer).await?;

    if first_chunk_size == 0 {
        return Err(DbxUpError::Io(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "File is empty",
        )));
    }

    let first_chunk = Bytes::from(buffer[..first_chunk_size].to_vec());

    // Start upload session with first chunk
    let start_arg = files::UploadSessionStartArg::default();
    let session_result = files::upload_session_start(client.client(), &start_arg, first_chunk)
        .await
        .map_err(|e| DbxUpError::DropboxApi(format!("Failed to start session: {}", e)))?;

    let session_id = session_result.session_id;
    let mut offset = first_chunk_size as u64;

    // Upload remaining chunks
    loop {
        let mut buffer = vec![0u8; CHUNK_SIZE];
        let bytes_read = f.read(&mut buffer).await?;

        if bytes_read == 0 {
            // No more data, finish the upload
            break;
        }

        let chunk = Bytes::from(buffer[..bytes_read].to_vec());

        // Append chunk to session
        let cursor = files::UploadSessionCursor::new(session_id.clone(), offset);
        let append_arg = files::UploadSessionAppendArg::new(cursor);
        files::upload_session_append_v2(client.client(), &append_arg, chunk)
            .await
            .map_err(|e| DbxUpError::DropboxApi(format!("Failed to append chunk: {}", e)))?;

        offset += bytes_read as u64;
    }

    // Finish the upload session
    let cursor = files::UploadSessionCursor::new(session_id, offset);
    let commit_info = files::CommitInfo::new(file.dropbox_path.clone())
        .with_mode(files::WriteMode::Overwrite)
        .with_autorename(false)
        .with_mute(false);
    let finish_arg = files::UploadSessionFinishArg::new(cursor, commit_info);

    // Empty bytes for the final call
    let empty_bytes = Bytes::new();
    files::upload_session_finish(client.client(), &finish_arg, empty_bytes)
        .await
        .map_err(|e| DbxUpError::DropboxApi(format!("Failed to finish session: {}", e)))?;

    Ok(())
}
