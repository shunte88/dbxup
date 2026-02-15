# Persistent Upload Queue

## Overview

The watcher service now uses a **persistent upload queue** that survives service restarts, ensuring no uploads are lost even if the service crashes or is stopped.

## Features

### ✅ Queue Persistence
- Queue state saved to `~/.dbxup_queue.json`
- Auto-saves every 30 seconds
- Loads automatically on startup
- Merges with startup file scan

### ✅ Resumable Uploads
- Tracks upload session IDs for large files (>150MB)
- Can resume chunked uploads after restart
- Session validation (expires after 3 hours)
- Stale sessions automatically cleared

### ✅ Retry Logic
- Tracks retry count per file
- Automatically retries failed uploads
- Failed entries logged and can be pruned
- Max retries configurable (default: 5)

### ✅ Smart Deduplication
- Files already in queue are skipped
- Startup scan merges with persisted queue
- Size comparison prevents re-uploads

## Queue File Format

Location: `~/.dbxup_queue.json`

```json
[
  {
    "local_path": "/path/to/local/file.txt",
    "dropbox_path": "/remote/folder/file.txt",
    "size": 1024,
    "queued_at": {
      "secs_since_epoch": 1704067200,
      "nanos_since_epoch": 0
    },
    "upload_session": null,
    "retry_count": 0
  },
  {
    "local_path": "/path/to/large/file.bin",
    "dropbox_path": "/remote/folder/file.bin",
    "size": 209715200,
    "queued_at": {
      "secs_since_epoch": 1704067200,
      "nanos_since_epoch": 0
    },
    "upload_session": {
      "session_id": "AaBC123...",
      "uploaded_bytes": 104857600,
      "started_at": {
        "secs_since_epoch": 1704070800,
        "nanos_since_epoch": 0
      }
    },
    "retry_count": 0
  }
]
```

## How It Works

### On Startup

1. **Load Persistent Queue**
   ```
   Load ~/.dbxup_queue.json
   ├─ Validate entries (file still exists, size matches)
   ├─ Check upload sessions (clear if stale)
   └─ Build in-memory queue
   ```

2. **Startup File Scan**
   ```
   Scan watched folders
   ├─ Check if file in persistent queue → Skip
   ├─ Check if file in Dropbox
   │   ├─ Exists with same size → Skip
   │   └─ Missing or different size → Add to queue
   └─ Merge with persistent queue
   ```

3. **Begin Processing**
   - Background processor polls queue every second
   - Respects parallelism limit
   - Saves queue state every 30 seconds

### During Operation

```
New File Detected
    ↓
Check File Stability (debounce)
    ↓
Check if in Queue → Already queued
    ↓
Check Dropbox (size comparison)
    ↓
Add to Persistent Queue
    ↓
Auto-save (every 30s)
    ↓
Background Processor Picks Up
    ↓
Upload with Retry Logic
    ↓
Remove from Queue on Success
    ↓
Auto-save
```

### On Restart/Crash

```
Service Stops (gracefully or crash)
    ↓
Queue state on disk (last auto-save)
    ↓
Service Starts
    ↓
Load ~/.dbxup_queue.json
    ↓
Validate Entries
    ├─ Valid → Keep in queue
    └─ Invalid → Remove
    ↓
Perform Startup Scan
    ↓
Merge Queues (deduplicate)
    ↓
Resume Processing
```

## Upload Session Resume (Large Files)

For files ≥150MB using chunked uploads:

1. **During Upload**
   - Upload session ID stored in queue
   - Progress (uploaded_bytes) tracked
   - Timestamp recorded

2. **On Restart**
   - Load session from queue
   - Check if session is still valid (<3 hours old)
   - If valid:
     - Resume from `uploaded_bytes` offset
     - Continue chunked upload
   - If stale:
     - Clear session
     - Restart upload from beginning

3. **Session Expiry**
   - Dropbox sessions expire after 4 hours
   - We check after 3 hours to be safe
   - Stale sessions logged and cleared

## Queue Management

### Auto-Cleanup

- **Invalid entries**: Removed on load (file missing/changed)
- **Stale sessions**: Cleared on load (>3 hours old)
- **Failed uploads**: Removed after max retries (5 by default)

### Manual Cleanup

```bash
# View queue
cat ~/.dbxup_queue.json | jq

# Clear entire queue
rm ~/.dbxup_queue.json

# Restart service to rebuild from scan
sudo systemctl restart dbxup
```

## Logging

Queue-related log entries:

```
# On startup
INFO  Loaded 5 entries from persistent queue
INFO    2 entries have resumable upload sessions
INFO  Scanning existing files in watched folders...
INFO  Startup scan complete: 10 files found, 3 queued for upload

# During operation
DEBUG Saved queue state: 8 entries
INFO  File stable, queueing for upload: /path/to/file.txt
INFO  📤 Starting upload: /path/to/file.txt -> /remote/file.txt (1024 bytes)
INFO  ✓ Upload complete: /path/to/file.txt

# On errors
WARN  Removing invalid queue entry (file missing or changed): /path/to/gone.txt
INFO  Clearing stale upload session for: /path/to/old.bin
ERROR ✗ Upload failed: /path/to/problem.txt
ERROR Removing entry after 5 failed attempts: /path/to/problem.txt
```

## Benefits

### Reliability
- **No lost uploads**: Queue survives crashes and restarts
- **Automatic recovery**: Failed uploads retried automatically
- **Session resume**: Large files resume from where they left off

### Efficiency
- **Deduplication**: Files already queued or uploaded are skipped
- **Size comparison**: Only uploads when content changes
- **Merge logic**: Startup scan integrates with persisted state

### Visibility
- **Persistent state**: Queue file can be inspected anytime
- **Comprehensive logging**: All queue operations logged
- **Retry tracking**: Failed attempts tracked per file

## Testing Scenarios

### 1. Service Restart During Upload

```bash
# Start watcher with large file
dbxup watch --folders /local:/remote

# Add 200MB file
cp large_file.bin /local/

# Wait for upload to start (watch logs)
# Kill service mid-upload
sudo systemctl stop dbxup

# Restart
sudo systemctl start dbxup

# Expected: Upload resumes from where it left off
```

### 2. Service Crash with Pending Queue

```bash
# Queue multiple files
for i in {1..10}; do
  echo "content $i" > /local/file$i.txt
done

# Kill service (simulate crash)
sudo kill -9 $(pidof dbxup)

# Restart
sudo systemctl start dbxup

# Expected: All pending files still in queue, get uploaded
```

### 3. Invalid Queue Entries

```bash
# Stop service
sudo systemctl stop dbxup

# Manually modify queue file (corrupt entry)
vim ~/.dbxup_queue.json

# Restart service
sudo systemctl start dbxup

# Expected: Invalid entries logged and removed, valid entries processed
```

### 4. File Changes While Queued

```bash
# Add file to queue
echo "original" > /local/test.txt

# Wait for it to queue (before upload)

# Modify file
echo "modified content" > /local/test.txt

# Expected: File stability check resets, new version queued after stable
```

## Implementation Details

### Core Components

1. **PersistentQueue** (`src/persistent_queue.rs`)
   - In-memory HashMap of QueueEntry
   - Load/save from disk
   - Entry validation and pruning

2. **QueueEntry**
   - File metadata (paths, size)
   - Timestamp (queued_at)
   - Upload session info
   - Retry counter

3. **Background Tasks**
   - Upload processor (polls queue)
   - Queue saver (auto-saves every 30s)
   - Token refresh (refreshes hourly)

### Thread Safety

- Queue wrapped in `Arc<RwLock<WatcherState>>`
- Multiple readers, single writer
- Write lock only for queue modifications
- Read lock for queue inspection

### Error Handling

- Invalid entries: Removed on load
- Stale sessions: Cleared automatically
- Upload failures: Increment retry, keep in queue
- Max retries exceeded: Remove and log
- File disappeared: Remove from queue
- Size changed: File stability resets

## Configuration

Currently hardcoded, can be made configurable:

```rust
// In persistent_queue.rs
const SESSION_STALE_HOURS: u64 = 3;
const MAX_RETRIES: u32 = 5;

// In watcher.rs
const QUEUE_SAVE_INTERVAL: u64 = 30; // seconds
const QUEUE_POLL_INTERVAL: u64 = 1;  // seconds
```

## Future Enhancements

Potential improvements:

1. **Configurable retry strategy**
   - Exponential backoff
   - Per-file retry limits
   - Retry delay configuration

2. **Queue priorities**
   - Prioritize small files
   - User-defined priorities
   - Age-based ordering

3. **Progress tracking**
   - Real-time progress updates
   - Bandwidth tracking
   - ETA calculations

4. **Queue inspection API**
   - REST API for queue status
   - Web UI for queue management
   - Programmatic queue control

5. **Advanced session management**
   - Session keepalive
   - Multi-part upload optimization
   - Chunk-level retry

## Files Modified

- `src/persistent_queue.rs` - New module (300+ lines)
- `src/watcher.rs` - Refactored to use persistent queue
- `src/main.rs` - Added module declaration
