# Changelog

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
