# stormd

Container init system for scratch images. A single static binary that replaces shell, systemd, and cron inside minimal containers. SSH in, manage processes, tail logs — like a real Linux server.

## What it does

- **Process supervisor** — launches and monitors one or more binaries with configurable restart policies
- **SSH server** — built-in SSH with a bash-like management shell (process control, log tailing, tab completion)
- **VT100 terminals** — per-process terminal emulation, viewable via SSH, WebSocket, or web UI
- **Structured logging** — VT100 parsing, MinIO S3 storage, syslog receiver, broadcast streams
- **Web terminal** — browser-based terminal and log viewer at `/ui/terminal` and `/ui/logs`
- **TUI client** — `stormsh` standalone TUI for remote process management
- **Stdio capture** — captures stdout/stderr from each process through VT100 + structured storage
- **Log management** — rotation, query API, MinIO persistence, realtime WebSocket streaming
- **Cron scheduler** — run commands on cron schedules
- **REST API** — full control plane for status, process management, logs, terminals, and debug
- **Event system** — push events to NATS or webhooks when processes start/stop/crash
- **PID 1** — proper zombie reaping and signal handling for scratch containers

## Workspace structure

```
stormd/
  Cargo.toml                   # workspace root
  crates/
    stormd/                    # main binary — init, supervisor, API, SSH server
    stormlog/                  # library — VT100, MinIO storage, syslog, streams
    stormsh/                   # CLI — TUI console client
```

## Quick start

```bash
# Build all crates
cargo build --release

# Run stormd with config
./target/release/stormd --config /etc/stormd/config.toml

# SSH into the container
ssh root@container -p 22    # password: stormd

# Connect TUI client
./target/release/stormsh --host 192.168.1.100 --port 8080
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

## Configuration

See `config/example.toml` for a complete annotated example.

```toml
[general]
name = "my-service"
log_dir = "/var/log/stormd"

[api]
bind = "0.0.0.0:8080"

[ssh]
enabled = true
bind = "0.0.0.0:22"
password = "stormd"

[stormlog.minio]
enabled = true
endpoint = "http://127.0.0.1:9000"
bucket = "logs"

[stormlog.syslog]
enabled = true

[stormlog.terminal]
rows = 24
cols = 80

[[process]]
name = "miniminio"
command = "/miniminio"
args = ["--data-dir", "/data/minio"]
env = { MINIO_ROOT_USER = "stormd", MINIO_ROOT_PASSWORD = "stormdpass" }
on_failure = "restart"

[[process]]
name = "main-app"
command = "/app/server"
on_failure = "restart"
depends_on = ["miniminio"]

[[cron]]
name = "cleanup"
schedule = "0 0 * * * *"
command = "/app/cleanup"
```

## Container usage

```dockerfile
FROM scratch
COPY stormd /stormd
COPY miniminio /miniminio
COPY myapp /app/myapp
COPY config.toml /etc/stormd/config.toml
VOLUME /data/minio
VOLUME /var/log/stormd
EXPOSE 8080 9000 22
ENTRYPOINT ["/stormd"]
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
| GET | `/api/v1/logs/files` | List log files with sizes |
| GET | `/api/v1/logs/stored` | Query MinIO-stored logs |
| POST | `/api/v1/logs/ingest` | Structured log ingestion |
| GET | `/api/v1/cron` | List cron jobs with status |
| POST | `/api/v1/backup` | Trigger manual log backup |
| WS | `/ws/console/{process}` | Realtime terminal stream |
| WS | `/ws/logs` | Realtime log tailing |
| GET | `/ui/terminal` | Web terminal page |
| GET | `/ui/logs` | Web log viewer page |

## Process failure policies

| Policy | Behavior |
|--------|----------|
| `restart` | Restart after delay, up to `max_restarts` in `restart_window_secs` |
| `fail` | Fail the entire container (exit code 1) |
| `ignore` | Leave process stopped, container keeps running |

## Events

Events are emitted for process lifecycle changes and can be sent to NATS or webhooks:

- `container_starting`, `container_stopping`, `container_failing`
- `process_started`, `process_stopped`, `process_crashed`, `process_restarting`
- `cron_executed`, `cron_failed`
- `backup_started`, `backup_completed`, `backup_failed`

## Build

```bash
# Debug build (all crates)
cargo build

# Static musl release
cargo build --release --target x86_64-unknown-linux-musl

# ARM64 musl release
cargo build --release --target aarch64-unknown-linux-musl

# Without NATS support
cargo build --release --no-default-features
```

## Version

0.2.0
