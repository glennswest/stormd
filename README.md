# stormd

Container init system for scratch images. A single static binary that replaces shell, systemd, and cron inside minimal containers.

## What it does

- **Process supervisor** — launches and monitors one or more binaries with configurable restart policies
- **Stdio capture** — captures stdout/stderr from each process into structured log files
- **Log management** — rotation, query API, download, and backup/shipping on failure
- **Cron scheduler** — run commands on cron schedules
- **REST API** — full control plane for status, process management, logs, and debug
- **Event system** — push events to NATS or webhooks when processes start/stop/crash
- **PID 1** — proper zombie reaping and signal handling for scratch containers
- **Debug** — optional endpoints for signals, stdin injection, and runtime inspection

## Quick start

```bash
# Build static binary (musl)
cargo build --release --target x86_64-unknown-linux-musl

# Run with config
./stormd --config /etc/stormd/config.toml
```

## Configuration

See `config/example.toml` for a complete annotated example.

```toml
[general]
name = "my-service"
log_dir = "/var/log/stormd"

[api]
bind = "0.0.0.0:8080"

[[process]]
name = "main-app"
command = "/app/server"
on_failure = "restart"
restart_delay_secs = 5
max_restarts = 10

[[cron]]
name = "cleanup"
schedule = "0 0 * * * *"
command = "/app/cleanup"
```

## Container usage

```dockerfile
FROM scratch
COPY stormd /stormd
COPY myapp /app/myapp
COPY config.toml /etc/stormd/config.toml
VOLUME /var/log/stormd
EXPOSE 8080
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
| GET | `/api/v1/logs` | Query logs (`?process=X&tail=100&search=error`) |
| GET | `/api/v1/logs/{process}` | Process-specific logs |
| GET | `/api/v1/logs/files` | List log files with sizes |
| GET | `/api/v1/cron` | List cron jobs with status |
| POST | `/api/v1/backup` | Trigger manual log backup |
| GET | `/api/v1/debug/info` | Debug info (requires `debug.enabled`) |
| POST | `/api/v1/debug/processes/{name}/signal` | Send signal (requires `debug.allow_signal`) |
| POST | `/api/v1/debug/processes/{name}/stdin` | Write to stdin (requires `debug.allow_stdin`) |

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

## Log backup

When `backup.on_failure = true`, stormd archives all log files and ships them to the configured HTTP endpoint before exiting. Useful for post-mortem debugging of crashed containers.

## Build

```bash
# Debug build
cargo build

# Static musl release
cargo build --release --target x86_64-unknown-linux-musl

# Without NATS support
cargo build --release --no-default-features
```

## Version

0.1.0
