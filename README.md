# dbxup - Dropbox Upload CLI

A fast, reliable Rust CLI tool for uploading files to Dropbox with concurrent uploads, automatic OAuth token refresh, and folder watching for background sync.

## Features

✨ **Core Upload**
- Upload single files or entire directories
- Concurrent uploads (configurable parallelism)
- Automatic chunked upload for large files (≥150MB)
- Progress tracking and error reporting

🔐 **Smart Authentication**
- OAuth 2.0 with automatic token refresh
- Secure token storage in `~/.dbxup_tokens`
- No manual token management needed

👁️ **Watch Mode (NEW)**
- Monitor folders and auto-upload new files
- Run as background daemon/service
- Auto-reload config file when modified (no restart needed!)
- Robust handling of folder deletion/recreation
- Event debouncing to avoid duplicate uploads

## Quick Start

### 1. Install

```bash
cargo build --release
```

The binary will be at `./target/release/dbxup`

### 2. Set Up Authentication

Export your Dropbox app credentials:
```bash
export DROPBOX_APP_KEY="your_app_key_here"
export DROPBOX_APP_SECRET="your_app_secret_here"
```

Run the auth command:
```bash
./target/release/dbxup auth --app-key $DROPBOX_APP_KEY --app-secret $DROPBOX_APP_SECRET
```

This opens a browser for authorization. Copy the code and paste it back into the terminal. Your refresh token will be saved for future use.

### 3. Upload Files

**Single file:**
```bash
./target/release/dbxup --file /path/to/file.txt --destination /Uploads
```

**Entire directory:**
```bash
./target/release/dbxup --dir /path/to/folder --destination /Backups
```

**With custom parallelism:**
```bash
./target/release/dbxup --dir /path/to/folder --destination /Backups --parallel 10
```

## Commands

### `auth` - OAuth Authorization

Set up OAuth credentials with automatic refresh:

```bash
dbxup auth --app-key <KEY> --app-secret <SECRET>
```

This is a one-time setup. The tool will automatically refresh access tokens as needed.

### `setup` - Direct Refresh Token Setup

If you already have a refresh token:

```bash
dbxup setup --app-key <KEY> --refresh-token <TOKEN>
```

### `watch` - Monitor Folders (Daemon Mode)

Auto-upload new files from monitored folders:

**Using config file (recommended):**
```bash
dbxup watch --config watch-config.json
```

**Using command-line:**
```bash
dbxup watch \
  --folders /local/path:/dropbox/path \
  --folders /another/path:/another/dest \
  --parallel 5 \
  --debounce 2000
```

See [WATCH_MODE.md](WATCH_MODE.md) for detailed documentation.

### `logout` - Clear Saved Tokens

Remove saved OAuth credentials:

```bash
dbxup logout
```

## CLI Options

### Upload Mode (default)

```
dbxup [OPTIONS]

OPTIONS:
  -t, --token <TOKEN>              Dropbox access token (or use DROPBOX_ACCESS_TOKEN)
  --app-key <KEY>                  Dropbox app key (or use DROPBOX_APP_KEY)
  --app-secret <SECRET>            Dropbox app secret (or use DROPBOX_APP_SECRET)

  -f, --file <FILE>                Upload a single file
  -d, --dir <DIR>                  Upload a directory (all files recursively)
  -o, --destination <PATH>         Dropbox destination folder [default: /]

  -p, --parallel <N>               Number of concurrent uploads [default: 5]
  --chunk-size <MB>                Chunk size in MB for large files [default: 8]

  -v, --verbose                    Enable verbose logging
  -h, --help                       Print help
```

### Watch Mode

```
dbxup watch [OPTIONS]

OPTIONS:
  -c, --config <FILE>              Config file with folders to watch (JSON)
  -f, --folders <PATH:PATH>...     Folders to watch (format: /local:/dropbox)
  -p, --parallel <N>               Number of concurrent uploads
  --debounce <MS>                  Debounce delay in milliseconds
```

## Configuration

### Environment Variables

Add to `~/.bashrc` or `~/.zshrc`:

```bash
export DROPBOX_APP_KEY="your_app_key_here"
export DROPBOX_APP_SECRET="your_app_secret_here"
```

### Watch Config File

Create `watch-config.json`:

```json
{
  "folders": [
    {
      "local_path": "/home/user/Documents",
      "dropbox_path": "/Backup/Documents",
      "description": "Auto-backup documents"
    },
    {
      "local_path": "/var/log/myapp",
      "dropbox_path": "/Logs/myapp",
      "description": "Application logs"
    }
  ],
  "settings": {
    "parallel_uploads": 5,
    "debounce_ms": 2000
  }
}
```

See `watch-config.example.json` for a complete example.

## Examples

### Example 1: Backup Photos

```bash
dbxup --dir ~/Pictures/2024 --destination /Photos/2024 --parallel 10
```

### Example 2: Upload Build Artifacts

```bash
dbxup --dir ./dist --destination /Archive/builds/$(date +%Y%m%d)
```

### Example 3: Large File Upload

```bash
# Files ≥150MB automatically use chunked upload
dbxup --file ./large-video.mp4 --destination /Videos --chunk-size 8
```

### Example 4: Watch Mode for Backups

```bash
# Set up once
dbxup auth

# Create config
cat > watch-config.json <<EOF
{
  "folders": [
    {"local_path": "/home/user/Documents", "dropbox_path": "/Backups/Documents"}
  ],
  "settings": {
    "parallel_uploads": 5,
    "debounce_ms": 3000
  }
}
EOF

# Run in background
dbxup watch --config watch-config.json
```

### Example 5: Watch Mode as systemd Service

See [WATCH_MODE.md](WATCH_MODE.md) for complete systemd setup instructions.

## How It Works

### Authentication Flow

1. **Initial Setup**: Run `dbxup auth` to get a refresh token via OAuth
2. **Token Storage**: Refresh token saved in `~/.dbxup_tokens`
3. **Automatic Refresh**: On each upload, tool checks if access token needs refresh
4. **Seamless**: No manual intervention required after initial setup

### Upload Strategy

- **Small files (<150MB)**: Single API call using `files/upload`
- **Large files (≥150MB)**: Chunked upload using upload session API
  - Start session with first chunk
  - Append remaining chunks (8MB default)
  - Finalize session

### Concurrency Control

Uses `tokio::sync::Semaphore` to limit concurrent uploads:
- Prevents overwhelming the API
- Controls network bandwidth usage
- Configurable with `--parallel` flag

### Watch Mode Operation

1. **File System Monitoring**: Uses `notify` crate for efficient file watching
2. **Event Debouncing**: Waits for debounce period to avoid partial uploads
3. **Folder Resilience**: Automatically re-watches folders that appear/reappear
4. **Concurrent Uploads**: Multiple files uploaded in parallel (configurable)

## Architecture

```
dbxup/
├── src/
│   ├── main.rs              # CLI entry point, command routing
│   ├── config.rs            # Configuration validation
│   ├── error.rs             # Error types
│   ├── oauth.rs             # OAuth flow, token refresh
│   ├── token_storage.rs     # Token persistence
│   ├── retry.rs             # Retry logic with backoff
│   ├── upload_manager.rs    # Concurrent upload orchestration
│   ├── watcher.rs           # Folder watching daemon
│   ├── dropbox/
│   │   ├── client.rs        # Dropbox API client wrapper
│   │   └── upload.rs        # Upload logic (simple & chunked)
│   └── files/
│       ├── scanner.rs       # Directory traversal
│       └── metadata.rs      # File metadata, path mapping
├── Cargo.toml
├── README.md
├── WATCH_MODE.md            # Detailed watch mode docs
├── watch-config.example.json
└── dbxup-watch.service      # systemd service template
```

## Error Handling

- **Network Errors**: Automatic retry with exponential backoff
- **Rate Limiting**: Respects `Retry-After` headers
- **Auth Errors**: Clear messages with instructions
- **File Errors**: Continues with other files, reports failures at end
- **Token Expiry**: Automatic refresh (transparent to user)

## Performance

- **Speed**: Concurrent uploads maximize throughput
- **Memory**: Efficient streaming for large files
- **Network**: Connection pooling, automatic retry
- **CPU**: Minimal overhead when watching folders

## Limitations

- **Max file size**: 350GB (Dropbox API limit)
- **No deletion sync**: Watch mode only uploads new files
- **One-way sync**: Local → Dropbox (no bidirectional sync)
- **No conflict resolution**: Last upload wins

## Troubleshooting

### "No saved tokens found"

Run `dbxup auth` to set up OAuth credentials first.

### "Invalid access token"

Your access token expired. The tool should automatically refresh, but if it fails:

```bash
dbxup logout
dbxup auth  # Re-authenticate
```

### "File not found" errors

Check that:
- File/directory exists
- You have read permissions
- Path is correct (use absolute paths if unsure)

### Watch mode not starting

Ensure environment variables are set:
```bash
export DROPBOX_APP_KEY="..."
export DROPBOX_APP_SECRET="..."
```

### Files not uploading in watch mode

- Check logs: `journalctl --user -u dbxup-watch -f` (if using systemd)
- Verify files aren't hidden (starting with `.`)
- Confirm debounce period has passed (default: 2s)

## Development

### Build

```bash
cargo build
```

### Run Tests

```bash
cargo test
```

### Debug Logging

```bash
export RUST_LOG=debug
cargo run -- --verbose --file test.txt --destination /
```

## Dependencies

- **clap**: CLI parsing
- **tokio**: Async runtime
- **reqwest**: HTTP client
- **dropbox-sdk**: Dropbox API
- **notify**: File system watching
- **walkdir**: Directory traversal
- **thiserror**: Error handling
- **serde/serde_json**: Configuration parsing

## License

This project is provided as-is for personal use.

## Contributing

This is a personal tool, but suggestions and improvements are welcome!

## Support

For Dropbox API issues, see: https://www.dropbox.com/developers/documentation

For app key/secret setup:
1. Go to https://www.dropbox.com/developers/apps
2. Create an app
3. Generate app key and secret
4. Add redirect URI: `http://localhost:8888/oauth/callback` (for local OAuth)
