# Changelog

## [Unreleased]

### 2026-03-17
- **fix:** MinIO storage init was called on a throwaway instance — logs never reached MinIO (bucket was always None)
- **feat:** Run segmentation — each process start/restart creates a new run_id, logs are stored per-run in MinIO
- **feat:** `GET /api/v1/logs/{process}/runs` endpoint to list all historical runs for a process
- **feat:** `run_id` query parameter on `GET /api/v1/logs/stored` to filter logs by specific run
- **feat:** Process start/exit markers in log stream for clear run boundaries
- **feat:** Web UI dashboard at `/ui/` with process management, status overview, and controls
- **feat:** ANSI escape code to HTML conversion — terminal and log output renders colors properly
- **feat:** Disk/mount usage display with human-readable sizes and usage bars (`/api/v1/mounts`)
- **feat:** Memory usage monitoring with RSS/VMS history chart (`/api/v1/memory/history`)
- **feat:** Restart timestamps exposed in process status API and dashboard
- **feat:** Navigation bar across all UI pages (Dashboard, Terminal, Logs)
- **fix:** Control characters no longer displayed as raw escape sequences in web UI
- **feat:** Local file logging — all stdout/stderr written to `{log_dir}/{process}.log` with size-based rotation

## [v0.3.0] — 2026-03-01

### Added
- **OCI image updater** — automatic image updates for supervised processes via stormpull
- **`[updater]` config section** — enable/disable, registry, poll interval, data/rootfs directories
- **`image` field on `[[process]]`** — OCI image reference to track (e.g. `"myapp:latest"`)
- **Blue/green rootfs pivot** — pull new image, stop process, swap rootfs dirs, start with new binary
- **OCI layer assembly** — multi-layer tar extraction with whiteout file handling (.wh.)
- **CMD/ENTRYPOINT extraction** — command derived from OCI image config when not explicitly set
- **REST API endpoints** — `GET /api/v1/updates`, `GET /api/v1/updates/{name}`, `POST /api/v1/updates/{name}/trigger`
- **Updater events** — UpdateCheckStarted, UpdateAvailable, UpdatePulling, UpdatePivoting, UpdateCompleted, UpdateFailed
- **`update_process_config()`** — supervisor method for hot-swapping process config
- **`register_process()`** — supervisor method for registering processes without starting them

### Changed
- `ProcessConfig.command` is now optional (defaults to empty string) — derived from image when `image` is set
- Process validation: either `command` or `image` must be set
- Processes with `image` set are managed by the updater (initial pull + ongoing polling), not `start_all()`

## [v0.2.0] — 2026-02-28

### Added
- **Workspace refactor** — split into `stormd`, `stormlog`, and `stormsh` crates
- **stormlog** — structured logging library with VT100 terminal emulation (`vt100`), MinIO S3 storage (`rust-s3`), broadcast stream multiplexing, and syslog receiver (UDP/TCP/Unix)
- **SSH server** — built-in SSH server (`russh`) with password auth, PTY support, and auto-generated host keys
- **Shell** — bash-like management shell with `ps`, `start`, `stop`, `restart`, `attach`, `logs`, `grep`, `cron`, `status`, `uptime`, `env`, `whoami`, `hostname`, `df`, `free`, `help`, `exit` commands; tab completion, command history, colorized output, pipe support (`logs | grep pattern`)
- **WebSocket endpoints** — `/ws/console/{process}` for realtime VT100 terminal streaming, `/ws/logs` for realtime log tailing with filters
- **Web terminal UI** — `/ui/terminal` with process selector and live output, `/ui/logs` with severity filtering and search
- **stormsh** — TUI client (`ratatui` + `crossterm`) with process list, terminal view, and log viewer; keybindings for process control
- **REST endpoints** — `POST /api/v1/logs/ingest` for structured log ingestion, `GET /api/v1/logs/stored` for MinIO log queries, `GET /api/v1/terminal/{process}` for screen snapshots
- **Config sections** — `[stormlog.minio]`, `[stormlog.syslog]`, `[stormlog.terminal]`, `[ssh]`
- **NATS output publishing** — log entries forwarded to `stormd.output.{process}.{stream}` subjects
- **Containerfile** — updated for miniminio, stormsh, SSH port 22

### Changed
- `LogManager` replaced by `Arc<StormLog>` throughout supervisor, cron, API, and main
- Process stdout/stderr now flows through VT100 terminal emulation before line splitting
- Workspace uses shared dependency versions via `[workspace.dependencies]`

### Removed
- `src/logger.rs` — replaced by stormlog crate

## [v0.1.0] — 2026-02-28

### Added
- Process supervisor with restart policies (restart/fail/ignore)
- Per-process stdio capture (stdout/stderr) to log files
- Log rotation (size-based with configurable file count)
- REST API for status, process control, log queries, cron, backup, debug
- Cron-like job scheduler with cron expression syntax
- Event system with NATS and webhook transports
- Log backup/shipping on container failure (tar.gz to HTTP endpoint)
- Debug endpoints (process signals, stdin injection, system info)
- PID 1 zombie reaper for scratch containers
- Dependency ordering between supervised processes
- System stats collection (uptime, memory, process counts)
- TOML configuration with validation
- Graceful shutdown with signal handling (SIGTERM/SIGINT)

## [Unreleased]

