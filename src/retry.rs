/*
 *  retry.rs
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
use std::time::Duration;
use tokio::time::sleep;

/// Configuration for retry behavior
pub struct RetryConfig {
    pub max_retries: u32,
    pub initial_delay_ms: u64,
    pub max_delay_ms: u64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_delay_ms: 1000,  // 1 second
            max_delay_ms: 60000,     // 60 seconds
        }
    }
}

/// Retry a fallible async operation with exponential backoff
pub async fn retry_with_backoff<F, Fut, T>(
    config: &RetryConfig,
    operation: F,
) -> Result<T>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let mut attempt = 0;
    let mut delay_ms = config.initial_delay_ms;

    loop {
        match operation().await {
            Ok(result) => return Ok(result),
            Err(e) => {
                attempt += 1;

                // Check if error is retryable
                if !is_retryable_error(&e) {
                    return Err(e);
                }

                // Check if we've exhausted retries
                if attempt > config.max_retries {
                    return Err(DbxUpError::UploadFailed {
                        retries: attempt - 1,
                        message: format!("{}", e),
                    });
                }

                // Log retry attempt
                eprintln!(
                    "  ⚠ Retry {}/{}: {} (waiting {}ms)",
                    attempt, config.max_retries, e, delay_ms
                );

                // Wait before retrying
                sleep(Duration::from_millis(delay_ms)).await;

                // Exponential backoff with cap
                delay_ms = (delay_ms * 2).min(config.max_delay_ms);
            }
        }
    }
}

/// Determine if an error is retryable
fn is_retryable_error(error: &DbxUpError) -> bool {
    match error {
        // Network errors are retryable
        DbxUpError::Io(_) => true,
        DbxUpError::Http(_) => true,

        // Some API errors are retryable (rate limits, transient failures)
        DbxUpError::DropboxApi(msg) => {
            // Check for rate limiting or server errors
            msg.contains("429") || // Too Many Requests
            msg.contains("500") || // Internal Server Error
            msg.contains("502") || // Bad Gateway
            msg.contains("503") || // Service Unavailable
            msg.contains("504") || // Gateway Timeout
            msg.contains("timeout") ||
            msg.contains("connection")
        }

        // Authentication and config errors are not retryable
        DbxUpError::Authentication(_) => false,
        DbxUpError::Config(_) => false,
        DbxUpError::FileNotFound(_) => false,
        DbxUpError::InvalidPath(_) => false,
        DbxUpError::FileTooLarge(_) => false,
        DbxUpError::UploadFailed { .. } => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_retryable_errors() {
        assert!(is_retryable_error(&DbxUpError::DropboxApi(
            "429 Too Many Requests".to_string()
        )));
        assert!(is_retryable_error(&DbxUpError::DropboxApi(
            "503 Service Unavailable".to_string()
        )));
        assert!(!is_retryable_error(&DbxUpError::Authentication(
            "Invalid token".to_string()
        )));
        assert!(!is_retryable_error(&DbxUpError::Config(
            "Bad config".to_string()
        )));
    }
}
