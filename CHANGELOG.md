# Changelog

## [Unreleased]

### 2026-03-18
- **feat:** Liveness probe for process health checking — HTTP and TCP probes with configurable interval, timeout, failure threshold, and initial delay
- **feat:** Automatic restart on liveness failure — SIGUSR1 grace period, then SIGKILL if still hung
- **feat:** LivenessCheckFailed event emitted on probe failure threshold breach
- **feat:** `ps` and `systemctl status` show liveness probe status and failure count
- **feat:** Busybox-style shell — 80+ built-in commands for scratch containers
- **feat:** File operations: ls, cat, head, tail, cp, mv, rm, mkdir, touch, chmod, chown, find, ln, stat, pwd, wc, du, readlink, file, sha256sum, tee
- **feat:** Network commands: ifconfig, ip addr/link/route, ping, curl/wget, netstat/ss, nslookup, hostname, route
- **feat:** System commands: mount, df, free, uname, date, id, kill, printenv, export, unset, sleep, echo, which, type, lsof
- **feat:** systemctl emulation — maps start/stop/restart/status/list-units/is-active/is-failed to supervisor
- **feat:** Text processing: grep (files), sort, uniq, cut, tr, sed, rev, base64, xxd, xargs
- **feat:** dmesg command — queries all process logs from stormlog
- **feat:** General piping — `cmd1 | cmd2 | cmd3` chains any commands (not just `| grep`)
- **feat:** Output redirection — `cmd > file` and `cmd >> file`
- **feat:** Tab completion for file paths and systemctl subcommands
- **refactor:** Shell module split into categorized submodules (proc, log, file, net, sys, text)
- **fix:** Follow checkbox in logs UI now properly stops auto-scroll when unchecked
- **feat:** Stream filter dropdown (All/stdout/stderr) in logs UI toolbar

### 2026-03-17
- **feat:** `crashes` counter on process status — counts non-zero exits separately from restarts
- **feat:** Dashboard "Failed" stat renamed to "Crashes" showing total crash count, not just current state
- **feat:** Restart history entries link to logs page filtered by process
- **feat:** Logs page accepts `?process=` query param for deep linking from dashboard
- **feat:** Log severity auto-detection — PANIC/FATAL/SEGFAULT → Emergency, CRITICAL → Critical, ERROR → Error, WARN → Warning
- **feat:** Process crash emits `*** PROCESS CRASHED ***` at Emergency severity — visible with severity filter
- **fix:** Nav "stormd" text made more visible (was too faded)
- **fix:** Mount dedup changed from device to mount_point — PVC mounts now visible in Kubernetes
- **feat:** Nav bar shows container name as brand on left, "stormd" on right
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
- **feat:** `on_exit` config option — controls behavior on clean exit (exit code 0): `restart` (default) or `stop`
- **change:** `restart_delay_secs` default changed from 5 to 1
- **feat:** Log archival to MinIO on process exit — local log file uploaded as `archive/{process}/{run_id}/{failed|exited}.log`, then removed from local disk
- **feat:** Failed vs clean exit logs distinguished in MinIO archive path (`failed.log` vs `exited.log`)
- **change:** Default API port changed from 8080 to 9080 to avoid conflicts
- **fix:** Enable ICMP echo replies and network sysctls at startup for veth-based container networking
- **fix:** Segfault/panic — `blocking_lock()` called from async context in `spawn_capture` caused tokio runtime panic; changed to async `lock().await`
- **feat:** Run selector in Logs UI — browse historical runs from MinIO or local archives, with failed/exited tags
- **feat:** Last 100 lines of recent logs loaded on page open in Logs UI
- **feat:** `/api/v1/logs/files/{filename}` endpoint to read specific archived log files
- **fix:** Mount display filtered to real filesystems only (no pseudo-fs, cgroups, overlays deduplicated)
- **fix:** Mount display reformatted as table with columns for mount, device, type, used, total, free, usage bar
- **fix:** Reader tasks awaited (5s timeout) before archiving logs on process exit — no more lost stderr on crash

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

