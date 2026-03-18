# stormd

Container init system for scratch images. A single static binary that replaces shell, systemd, and cron inside minimal containers. SSH in, manage processes, tail logs, view a web dashboard — like a real Linux server, in 9 MB.

## What it does

- **Process supervisor** — launches and monitors one or more binaries with configurable restart policies
- **Web dashboard** — browser-based process management, memory charts, mount usage, restart history at `/ui/`
- **SSH server** — built-in SSH with a bash-like management shell (process control, log tailing, tab completion)
- **VT100 terminals** — per-process terminal emulation, viewable via SSH, WebSocket, or web UI
- **Structured logging** — severity auto-detection, MinIO S3 storage, syslog receiver, broadcast streams
- **Log archival** — process logs archived to MinIO on exit with failed/exited distinction, run history browsable in UI
- **Stdio capture** — captures stdout/stderr with automatic severity detection (PANIC/FATAL/ERROR/WARN)
- **Cron scheduler** — run commands on cron schedules
- **OCI image updater** — automatic image updates with blue/green rootfs pivot via stormpull
- **REST API** — full control plane for status, process management, logs, terminals, and debug
- **Event system** — push events to NATS or webhooks when processes start/stop/crash
- **PID 1** — proper zombie reaping, signal handling, and network sysctl init for scratch containers

## Workspace structure

```
stormd/
  Cargo.toml                   # workspace root
  crates/
    stormd/                    # main binary — init, supervisor, API, SSH server, web UI
    stormlog/                  # library — VT100, file logging, MinIO storage, syslog, streams
    stormsh/                   # CLI — TUI console client
```

## Quick start

### Build

```bash
# Debug build (all crates)
cargo build

# Static musl release (x86_64)
cargo build --release --target x86_64-unknown-linux-musl

# Static musl release (ARM64 — for MikroTik, Raspberry Pi, etc.)
cargo build --release --target aarch64-unknown-linux-musl

# Without NATS support
cargo build --release --no-default-features
```

### Run

```bash
./target/release/stormd --config /etc/stormd/config.toml
```

### Access

```bash
# Web dashboard
open http://localhost:9080/ui/

# SSH into the container (default password: stormd)
ssh root@localhost -p 22

# REST API
curl http://localhost:9080/api/v1/status | jq
```

## Example: scratch container with your app

This example shows how to package stormd with your own application binary in a scratch container. No OS, no shell, no package manager — just your binary and stormd.

### 1. Build stormd and your app

```bash
# Build stormd for your target architecture
cargo build --release --target aarch64-unknown-linux-musl

# Build your app as a static binary too
# (your app's build process here)
```

### 2. Write a config file

Create `config.toml`:

```toml
[general]
name = "my-service"
log_dir = "/var/stormd/logs"

[api]
bind = "0.0.0.0:9080"

[ssh]
enabled = true
bind = "0.0.0.0:22"
password = "changeme"

[stormlog.terminal]
rows = 50
cols = 120

[[process]]
name = "my-app"
command = "/app/server"
args = ["--port", "8080"]
env = { DATABASE_URL = "postgres://db:5432/mydb", LOG_LEVEL = "info" }
on_failure = "restart"          # restart on crash
on_exit = "restart"             # restart on clean exit too (long-running service)
restart_delay_secs = 2
max_restarts = 50
restart_window_secs = 3600
```

### 3. Write a Containerfile

```dockerfile
FROM scratch
COPY stormd /stormd
COPY my-app /app/server
COPY config.toml /etc/stormd/config.toml
EXPOSE 9080 8080 22
ENTRYPOINT ["/stormd"]
```

### 4. Build and run the container

```bash
# Build with podman (or docker)
podman build -t my-service:latest .

# Run it
podman run -d --name my-service \
  -p 9080:9080 \
  -p 8080:8080 \
  -p 2222:22 \
  -v my-service-data:/var/stormd \
  my-service:latest
```

### 5. Manage it

```bash
# Open the web dashboard
open http://localhost:9080/ui/

# SSH in and manage processes
ssh root@localhost -p 2222
# password: changeme

# Inside the SSH shell:
ps                    # list processes
logs my-app           # view recent logs
logs -f my-app        # follow logs in realtime
restart my-app        # restart the process
status                # full system status
```

### Multi-process example

stormd can supervise multiple processes with dependency ordering:

```toml
[general]
name = "full-stack"
log_dir = "/var/stormd/logs"

[api]
bind = "0.0.0.0:9080"

[ssh]
enabled = true
bind = "0.0.0.0:22"
password = "changeme"

# MinIO for log storage (optional)
[stormlog.minio]
enabled = true
endpoint = "http://127.0.0.1:9000"
bucket = "logs"
access_key = "stormd"
secret_key = "stormdpass"

# Start MinIO first for log storage
[[process]]
name = "minio"
command = "/miniminio"
args = ["--data-dir", "/data/minio"]
env = { MINIO_ROOT_USER = "stormd", MINIO_ROOT_PASSWORD = "stormdpass" }
on_failure = "restart"

# Main application depends on MinIO
[[process]]
name = "api-server"
command = "/app/server"
args = ["--port", "8080"]
on_failure = "restart"
on_exit = "restart"
depends_on = ["minio"]

# Worker process depends on main API
[[process]]
name = "worker"
command = "/app/worker"
args = ["--concurrency", "4"]
on_failure = "restart"
on_exit = "stop"                # don't restart workers on clean exit
depends_on = ["api-server"]

# Periodic cleanup job
[[cron]]
name = "cleanup"
schedule = "0 0 * * * *"       # every hour
command = "/app/cleanup"
```

```dockerfile
FROM scratch
COPY stormd /stormd
COPY miniminio /miniminio
COPY server /app/server
COPY worker /app/worker
COPY cleanup /app/cleanup
COPY config.toml /etc/stormd/config.toml
VOLUME /data/minio
EXPOSE 9080 8080 9000 22
ENTRYPOINT ["/stormd"]
```

## SSH shell commands

```
ps              — list supervised processes (colored status)
start <name>    — start a process
stop <name>     — stop a process
restart <name>  — restart a process
attach <name>   — attach to process VT100 terminal
logs [name]     — show recent logs
logs -f [name]  — follow logs realtime
grep <pattern>  — search logs
cron            — list cron jobs
status          — full system status
uptime          — container uptime
env             — environment variables
whoami          — current user (root)
hostname        — container name
df              — storage usage
free            — memory info
help            — list commands
exit            — close SSH session
```

Shell features: tab completion, command history, colorized output, piping (`logs | grep error`).

## Web UI

| Page | URL | Description |
|------|-----|-------------|
| Dashboard | `/ui/` | Process table, stats (uptime, avg uptime, restarts, memory), memory chart, mount usage, restart history |
| Terminal | `/ui/terminal` | Live VT100 terminal output per process |
| Logs | `/ui/logs` | Log viewer with severity filter, search, run selector for crash history |

The dashboard shows container name as the brand. Restart history entries link directly to the failed run's logs.

## Configuration reference

```toml
[general]
name = "my-service"                    # container name (shown in UI nav)
log_dir = "/var/stormd/logs"           # log file directory

[api]
bind = "0.0.0.0:9080"                 # REST API + web UI bind address

[ssh]
enabled = true                         # enable built-in SSH server
bind = "0.0.0.0:22"                   # SSH bind address
password = "stormd"                    # SSH password (default: stormd)

[events]
enabled = false                        # enable event system
# nats_url = "nats://localhost:4222"   # NATS server URL
# webhook_url = "http://..."           # webhook endpoint

[backup]
enabled = false                        # enable log backup on failure
on_failure = true                      # backup logs when container fails

[debug]
enabled = false                        # enable debug endpoints
allow_signal = false                   # allow sending signals via API
allow_stdin = false                    # allow stdin injection via API

[updater]
enabled = false                        # enable OCI image updater
# registry = "registry.example.com"
# poll_interval_secs = 300

[stormlog.minio]
enabled = false                        # enable MinIO log storage
endpoint = "http://127.0.0.1:9000"
bucket = "logs"
access_key = "stormd"
secret_key = "stormdpass"

[stormlog.syslog]
enabled = false                        # enable syslog receiver

[stormlog.terminal]
rows = 24                              # VT100 terminal rows
cols = 80                              # VT100 terminal columns
scrollback = 1000                      # scrollback buffer size

[stormlog.file]
max_size_bytes = 104857600             # 100 MiB per log file before rotation
max_files = 10                         # rotated files to keep

[[process]]
name = "my-app"                        # process name (must be unique)
command = "/app/server"                # binary path
args = ["--flag", "value"]             # command arguments
env = { KEY = "value" }                # environment variables
working_dir = "/"                      # working directory
on_failure = "restart"                 # restart | fail | ignore
on_exit = "restart"                    # restart | stop (for clean exit code 0)
restart_delay_secs = 1                 # delay before restart
max_restarts = 100                     # max restarts in window before failing
restart_window_secs = 3600             # restart counting window
depends_on = ["other-process"]         # start after these processes
# image = "myapp:latest"              # OCI image (for updater)

[[cron]]
name = "cleanup"                       # job name
schedule = "0 0 * * * *"              # cron expression (6-field with seconds)
command = "/app/cleanup"               # command to run
args = []                              # arguments
```

## REST API

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/v1/health` | Health check |
| GET | `/api/v1/status` | Full status (processes, cron, stats) |
| GET | `/api/v1/stats` | System stats (uptime, memory, counts) |
| GET | `/api/v1/processes` | List all processes |
| GET | `/api/v1/processes/{name}` | Get process status |
| POST | `/api/v1/processes/{name}/start` | Start a process |
| POST | `/api/v1/processes/{name}/stop` | Stop a process |
| POST | `/api/v1/processes/{name}/restart` | Restart a process |
| GET | `/api/v1/terminal/{process}` | VT100 screen snapshot |
| GET | `/api/v1/logs` | Query logs (`?process=X&tail=100&search=error`) |
| GET | `/api/v1/logs/{process}` | Process-specific logs |
| GET | `/api/v1/logs/{process}/runs` | List historical runs for a process |
| GET | `/api/v1/logs/files` | List archived log files with sizes |
| GET | `/api/v1/logs/files/{filename}` | Read a specific archived log file |
| GET | `/api/v1/logs/stored` | Query MinIO-stored logs (`?run_id=X`) |
| POST | `/api/v1/logs/ingest` | Structured log ingestion |
| GET | `/api/v1/mounts` | Disk/mount usage |
| GET | `/api/v1/memory/history` | Memory RSS/VMS history samples |
| GET | `/api/v1/cron` | List cron jobs with status |
| GET | `/api/v1/updates` | List OCI image update status |
| POST | `/api/v1/updates/{name}/trigger` | Trigger image update check |
| POST | `/api/v1/backup` | Trigger manual log backup |
| WS | `/ws/console/{process}` | Realtime terminal stream |
| WS | `/ws/logs` | Realtime log tailing (`?process=X&severity=error`) |

## Process failure policies

| `on_failure` | Behavior (non-zero exit) |
|--------------|--------------------------|
| `restart` | Restart after delay, up to `max_restarts` in `restart_window_secs`, then fail container |
| `fail` | Fail the entire container immediately (exit code 1) |
| `ignore` | Leave process stopped, container keeps running |

| `on_exit` | Behavior (clean exit, code 0) |
|-----------|-------------------------------|
| `restart` | Restart the process (default — for long-running services) |
| `stop` | Leave process stopped (for one-shot tasks) |

## Log severity detection

stormd automatically detects severity from log line content:

| Pattern | Severity |
|---------|----------|
| `PANIC`, `FATAL`, `SEGFAULT`, `SIGSEGV`, `SIGABRT`, `CORE DUMPED` | Emergency |
| `CRITICAL` | Critical |
| `ERROR:`, `[ERROR]`, `level=error` | Error |
| `WARNING:`, `[WARN`, `level=warn` | Warning |
| All other stdout | Info |
| All other stderr | Warning |

Process crashes emit a `*** PROCESS CRASHED ***` entry at Emergency severity.

## Events

Events are emitted for process lifecycle changes and can be sent to NATS or webhooks:

- `container_starting`, `container_stopping`, `container_failing`
- `process_started`, `process_stopped`, `process_crashed`, `process_restarting`
- `update_check_started`, `update_available`, `update_pulling`, `update_pivoting`, `update_completed`, `update_failed`
- `cron_executed`, `cron_failed`
- `backup_started`, `backup_completed`, `backup_failed`

## Version

0.3.0
