# Watch Mode - Auto-Upload New Files

## Overview

Watch mode monitors one or more local folders and automatically uploads new files to Dropbox as they appear. This runs as a daemon process, making it ideal for background sync, backup, or log collection scenarios.

## Prerequisites

1. **OAuth Setup**: Run `dbxup auth` first to set up authentication:
   ```bash
   export DROPBOX_APP_KEY="your_app_key"
   export DROPBOX_APP_SECRET="your_app_secret"
   ./target/release/dbxup auth --app-key $DROPBOX_APP_KEY --app-secret $DROPBOX_APP_SECRET
   ```

2. **Environment Variables**: Watch mode requires app credentials in environment:
   ```bash
   export DROPBOX_APP_KEY="your_app_key"
   export DROPBOX_APP_SECRET="your_app_secret"
   ```

## Usage

### Option 1: Configuration File (Recommended)

Create a JSON configuration file (e.g., `watch-config.json`):

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

Run with:
```bash
./target/release/dbxup watch --config watch-config.json
```

### Option 2: Command-Line Folders

Specify folders directly on the command line:
```bash
./target/release/dbxup watch \
  --folders /path/to/local:/dropbox/destination \
  --folders /another/path:/another/dropbox/path \
  --parallel 5 \
  --debounce 2000
```

## Configuration Options

### Folder Settings
- **local_path**: Local directory to monitor (will be created if it doesn't exist)
- **dropbox_path**: Destination folder in Dropbox
- **description**: Optional description for documentation

### Global Settings
- **parallel_uploads** (default: 5): Number of concurrent uploads
- **debounce_ms** (default: 2000): Delay in milliseconds before uploading (avoids duplicate uploads)

Command-line flags override config file settings:
- `--parallel <N>`: Override parallel uploads
- `--debounce <MS>`: Override debounce delay

## Robustness Features

### Config File Auto-Reload (NEW!)
When using a config file (`--config`), the watcher automatically detects changes and reloads:
- **No restart needed**: Edit the config file and changes take effect immediately
- **Add/remove folders**: Update the `folders` array in your config
- **Adjust settings**: Change `parallel_uploads` or `debounce_ms` on the fly
- **Safe reload**: If the config is invalid, the old config continues running

Example workflow:
```bash
# Start watcher with config
dbxup watch --config watch-config.json

# In another terminal, edit the config
vim watch-config.json  # Add a new folder

# Save the file - watcher detects change and reloads automatically!
# You'll see: "🔄 Config reloaded! Now watching X folder(s)"
```

This is perfect for long-running services - no need to restart when you want to monitor additional folders!

**Note**: When running as a systemd service, you can edit the config file at any time and the service will automatically reload without using `systemctl restart`. Just edit and save!

### Missing Folders
If a monitored folder doesn't exist when the watcher starts:
- A warning is logged
- The watcher continues monitoring other folders
- Every 30 seconds, it rechecks for missing folders
- When a folder reappears, monitoring resumes automatically

### Folder Deletion
If a monitored folder is deleted or unmounted while running:
- The event is logged
- Monitoring continues for other folders
- The folder will be re-monitored when it reappears (30s check interval)

### File Filtering
The watcher automatically:
- Ignores hidden files (starting with `.`)
- Skips non-regular files (directories, symlinks, etc.)
- Skips files over 350GB (Dropbox limit)

### Debouncing
Files are uploaded after the debounce period to avoid:
- Duplicate uploads during file writes
- Uploading partially-written files
- Overwhelming the API with rapid file changes

## Running as a Service

### systemd Service

1. **Edit the service file** (`dbxup-watch.service`):
   ```ini
   [Unit]
   Description=Dropbox Folder Watcher - Auto-upload new files
   After=network-online.target
   Wants=network-online.target

   [Service]
   Type=simple
   User=your_username
   WorkingDirectory=/home/your_username

   Environment="DROPBOX_APP_KEY=your_app_key_here"
   Environment="DROPBOX_APP_SECRET=your_app_secret_here"

   ExecStart=/data2/dbxup/target/release/dbxup watch \
       --config /path/to/watch-config.json

   Restart=always
   RestartSec=10

   StandardOutput=journal
   StandardError=journal
   SyslogIdentifier=dbxup-watch

   NoNewPrivileges=true
   PrivateTmp=true

   [Install]
   WantedBy=default.target
   ```

2. **Install and enable**:
   ```bash
   # Copy service file
   sudo cp dbxup-watch.service /etc/systemd/system/dbxup-watch@.service

   # Enable for your user
   systemctl --user enable dbxup-watch
   systemctl --user start dbxup-watch

   # Check status
   systemctl --user status dbxup-watch

   # View logs
   journalctl --user -u dbxup-watch -f
   ```

## Logging

Watch mode uses structured logging:
- **INFO**: Normal operations (file uploads, folder status)
- **WARN**: Non-critical issues (missing folders, temporary failures)
- **ERROR**: Critical failures (authentication, permanent errors)

Set log level via `RUST_LOG` environment variable:
```bash
export RUST_LOG=info  # or: debug, warn, error
./target/release/dbxup watch --config watch-config.json
```

In systemd, logs go to the journal:
```bash
# Follow logs in real-time
journalctl --user -u dbxup-watch -f

# Show recent logs
journalctl --user -u dbxup-watch -n 100

# Filter by priority
journalctl --user -u dbxup-watch -p err  # Errors only
```

## Examples

### Example 1: Backup Documents and Photos
```json
{
  "folders": [
    {
      "local_path": "/home/user/Documents",
      "dropbox_path": "/Backups/Documents"
    },
    {
      "local_path": "/home/user/Pictures",
      "dropbox_path": "/Backups/Pictures"
    }
  ],
  "settings": {
    "parallel_uploads": 3,
    "debounce_ms": 5000
  }
}
```

### Example 2: Application Logs
```json
{
  "folders": [
    {
      "local_path": "/var/log/nginx",
      "dropbox_path": "/Logs/nginx"
    },
    {
      "local_path": "/var/log/myapp",
      "dropbox_path": "/Logs/myapp"
    }
  ],
  "settings": {
    "parallel_uploads": 10,
    "debounce_ms": 1000
  }
}
```

### Example 3: Development Artifacts
```bash
# Monitor build outputs
./target/release/dbxup watch \
  --folders /home/user/projects/myapp/dist:/Archive/myapp/builds \
  --parallel 5 \
  --debounce 3000
```

## Troubleshooting

### "No saved tokens found"
Run `dbxup auth` first to set up OAuth credentials.

### "DROPBOX_APP_KEY env var required"
Export your app credentials:
```bash
export DROPBOX_APP_KEY="your_key"
export DROPBOX_APP_SECRET="your_secret"
```

Consider adding these to `~/.bashrc` or the systemd service file.

### Folder not being watched
Check logs for errors. The folder may not exist or may lack read permissions.
The watcher will automatically retry every 30 seconds.

### Files not uploading
- Check that files aren't hidden (starting with `.`)
- Verify the debounce period has passed (default: 2 seconds)
- Look for errors in logs related to file access or Dropbox API

### High CPU usage
Reduce the polling interval by monitoring fewer folders or increasing the debounce delay.

## Security Considerations

1. **Token Storage**: OAuth tokens are stored in `~/.dbxup_tokens` with user-only permissions
2. **Environment Variables**: Keep app credentials secure (use systemd Environment= or EnvironmentFile=)
3. **File Permissions**: The watcher runs with your user permissions; ensure folders are readable
4. **Network Security**: All API calls use HTTPS

## Performance

- **Startup**: Sub-second for most configurations
- **CPU**: Minimal when idle; spikes briefly during file events
- **Memory**: ~10-20MB base + additional per concurrent upload
- **Network**: Depends on upload volume; respects Dropbox rate limits

## Limitations

- Maximum file size: 350GB (Dropbox limit)
- No file deletion sync (only uploads new files)
- No bidirectional sync (one-way: local → Dropbox)
- Requires network connectivity (fails gracefully, retries automatically)
