# Changelog

## [Unreleased]
<!-- New unreleased changes go here -->

## [v0.5.0] — 2026-08-26

### 2026-08-26
- **feat:** themes — Storm (default), Midnight, Nord, Solar, Phosphor, and
  Light, picked from the nav bar and remembered per browser. A theme is one
  block of CSS token overrides: the ANSI palette for rendered process output
  and the memory chart re-color with it.
- **feat:** login system, off by default. `[api] password` (interactive) or
  `auth_token` (bearer) turns it on: login screen in the UI, HttpOnly
  in-memory sessions (24h), constant-time comparisons, and middleware that
  guards everything except health, metrics, the auth endpoints and static
  assets — the plugin proxy included. stormsh sends the token via
  `-t`/`--token` or `STORMD_TOKEN`.
- **feat:** typed component relations — `has_one`, `has_many`, `belongs_to`
  edges between component ids in the feed — and a relational grid view on
  the dashboard: nested grids along downward edges, sortable columns,
  multi-select with bulk start/stop/restart, and `has_many` edges as
  "select from a relationship" pickers. Cards grew relation chips. The
  `DataGrid`/`RelationPicker` Svelte components know nothing about stormd,
  in prep for stormdrive and stormconsole.
- **refactor:** the view contract (`ComponentSummary`, `Metric`, `Action`,
  `Relation`, `Health`, format helpers) moved to the shared
  [`stormview`](https://github.com/glennswest/stormview) crate; stormd
  assembles and serves it, stormsh deserializes the same types, so the two
  cannot disagree about the wire.
- **fix:** stormsh's default port is 9080 — stormd's actual API default —
  instead of 8080.

## [v0.4.0] — 2026-08-26

Everything since v0.3.0 (2026-03-01), kept in dated form below. Highlights:
the component-summary contract and the two dashboards that render it (Svelte
web UI, stormsh tiles); liveness probes; the busybox multi-call binary and
stormdbase image; CloudID auth and SFTP; run-segmented logging; the OCI image
updater; the stormcast log wire.

### Breaking
- The object-store half of `stormlog` and the syslog receivers are gone —
  the `s3` feature and the `[stormlog.minio]` / `[stormlog.syslog]` config
  sections no longer exist (see 2026-08-25 below).
- Default API port moved from 8080 to 9080 (see 2026-03-17 below).

### 2026-08-26
- **feat:** a plugin can report its own summary — `[process.ui] summary` names
  a URL returning JSON with any of `health`, `detail`, `metrics`, merged into
  the plugin's dashboard card in both UIs (best-effort, 400ms timeout,
  fetched concurrently).
- **feat:** stormsh opens on a Dashboard view — the same component summaries
  the web dashboard renders, drawn as a grid of tiles with health, detail,
  metrics, and the enabled actions on the selected tile (s/r/x, u for image
  updates, Enter to jump into a process's terminal). Views renumber to
  1:Dashboard 2:Processes 3:Terminal 4:Logs.
- **feat:** the web UI is a Svelte 5 SPA (`web/`), built to `web/dist`
  (committed) and embedded in the binary — `web.rs` goes from 1,176 lines of
  format!()-escaped HTML to a static file handler. The dashboard renders the
  component feed generically; logs (live tail, stored runs, local archives,
  crash links), terminal, per-process, and plugin-iframe views keep parity
  with the old pages. Old page URLs (`/ui/logs`, `/ui/terminal`,
  `/ui/ext/{name}`) redirect to their hash routes. 24 KB gzipped; no node at
  runtime; the look lives in one design-token block.
- **feat:** the component summary contract — `GET /api/v1/components` reports
  every part of the system (the supervisor, each process and plugin, cron
  jobs, mounts, logs, tracked images) in one uniform shape: id, kind, label,
  health, a one-line detail, headline metrics, and invocable actions.
  `/ws/components` pushes the full list whenever it changes. This feed is
  what both dashboards — web and stormsh — render from, so a subsystem
  implements one summary in Rust and appears in both UIs.

### 2026-08-25
- **refactor:** the log wire — severities, RFC 5424 framing, the multicast
  socket, escape stripping and the two volume limits — moved to
  [`stormcast`](https://github.com/glennswest/stormcast), shared with
  `stormpump`. It was written out in both, and one of them was going to drift;
  the drift shows up as a viewer that cannot read a node.
- **BREAKING:** removed the object-store half of `stormlog` — the MinIO client,
  bucket, credentials, entry buffer and flush loop — and the syslog receivers
  on UDP, TCP and `/dev/log`. Receiving, storing and indexing a fleet's logs is
  a collector's job (`mcastsyslog`); doing it in a container's init as well
  meant two stores, two schemas and a view that saw half the nodes, and it made
  the logs a node keeps depend on a service elsewhere being up — when the logs
  anyone wants are from the failure that took the network out. The `s3` feature
  and the `[stormlog.minio]` / `[stormlog.syslog]` config sections are gone.
- **feat:** `stormlog::store` answers queries and lists runs by reading the log
  files back, so `/api/v1/logs/stored` and the run picker work with no service
  behind them. The on-disk line format is written and parsed side by side, for
  the same reason the wire moved to one crate.
- **feat:** finished runs are pruned per process (`file.max_runs`, default 10).
  A process restarting in a loop writes one archive per restart, and without
  this the thing that fills the log volume is the record of what went wrong.
- **fix:** following with `severity=error` compared the name for equality, so a
  viewer watching for errors was not shown emergencies — the filter hid exactly
  what it existed to surface. It now means "at least this severe".
- **fix:** a WebSocket follower that fell behind had its dropped lines skipped
  in silence. The gap is now reported in the stream.

### 2026-08-19
- **fix:** the applet symlinks are relative (`../stormd`, `../../stormd`)
  instead of absolute (`/stormd`), in all three architecture Containerfiles.
  An absolute target only resolves when the image root is `/`. Under a
  copy-on-write model the rootfs is a clone mounted somewhere else — RouterOS
  will not take a block device as a container's root, so the image arrives at
  a mount point and the container's own root is a stub — and there `/stormd`
  does not exist. All 252 applet names then fail to resolve, which takes out
  `execve` for anything reached through them and reads as a missing binary
  rather than a broken link. This is what stopped the 2026-08-19 netwatch
  CoW trial: the container was created, the clone attached, and it exited
  immediately.
  Verified both ways: relative targets resolve at `/` and under `/payload`;
  the absolute form resolves at `/` and breaks the moment the image moves.
  Behaviour at `/` is unchanged, so this costs nothing for tarball-served
  containers.

### 2026-04-05
- **feat:** CloudID SSH public key auth — fetches authorized keys from CloudID metadata service (169.254.169.254) with 30s refresh; enable via `[ssh] owner = "my-tag"`
- **feat:** `cloudid_url` and `owner` config fields in `[ssh]` section for CloudID integration
- **feat:** Cloud ID — per-instance unique identifier accepted as SSH password; configurable via `[general] cloud_id`, `STORM_CLOUD_ID` env var, or auto-generated UUID persisted to `{log_dir}/.cloudid`
- **feat:** SFTP subsystem — built-in SFTP server enables `scp` and `sftp` file transfers into containers (open, read, write, stat, mkdir, rmdir, rename, symlink, readlink, remove)
- **feat:** `GET /api/v1/cloudid` endpoint — returns the instance cloud ID and container name

### 2026-03-21
- **feat:** Extensible plugin UI — managed processes can declare `[process.ui]` with `label` and `proxy` to add custom tabs to the stormd web UI
- **feat:** Reverse proxy at `/ui/proxy/{name}/` forwards all HTTP methods to the plugin's target URL (same-origin, no CORS issues)
- **feat:** Plugin pages rendered in iframe with stormd nav chrome — apps get full-page rendering with consistent navigation
- **feat:** `GET /api/v1/plugins` endpoint lists registered UI plugins
- **feat:** Dynamic nav bar — plugin tabs appear alongside Dashboard, Terminal, Logs automatically
- **feat:** `POST /api/v1/shutdown` endpoint — gracefully stops all supervised processes and exits stormd (PID 1), triggering container restart by external orchestrator
- **feat:** Optional `exitCode` parameter in shutdown request body — allows callers to distinguish update restarts from normal shutdowns

### 2026-03-18
- **feat:** Liveness probe for process health checking — HTTP and TCP probes with configurable interval, timeout, failure threshold, and initial delay
- **feat:** Automatic restart on liveness failure — SIGUSR1 grace period, then SIGKILL if still hung
- **feat:** LivenessCheckFailed event emitted on probe failure threshold breach
- **feat:** `ps` and `systemctl status` show liveness probe status and failure count
- **feat:** `liveness [name]` shell command — shows probe config, status, and failure counts
- **feat:** `status` command shows liveness health summary (N/M healthy)
- **feat:** `--healthcheck` CLI flag — probes running instance, exits 0/1 for Docker HEALTHCHECK in scratch
- **feat:** HEALTHCHECK instruction added to Containerfile (uses `/stormd --healthcheck`)
- **feat:** Containerfile.x86_64 for x86_64-unknown-linux-musl builds
- **feat:** OCI image labels (title, description, source, vendor) on container images
- **feat:** Multi-stage Containerfile — busybox symlinks baked into image at build time (63 commands in /bin, /usr/bin, /sbin, /usr/sbin)
- **feat:** `stormdbase` container image — 12.7 MB scratch image with all busybox commands pre-linked
- **feat:** Web UI dashboard: liveness column in process table + liveness stat card
- **feat:** Busybox multi-call binary — stormd serves as ls, cat, grep, ping, curl, etc. via argv[0] symlinks
- **feat:** 63 standalone commands available: file ops, network, system, text processing
- **feat:** `--install /bin` flag creates symlinks for all standalone commands
- **feat:** `--list-commands` flag prints all available standalone commands
- **feat:** Auto-installs busybox symlinks into /bin, /usr/bin, /sbin, /usr/sbin on daemon startup
- **feat:** Piped stdin supported in standalone mode (e.g., `echo foo | /bin/sort`)
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

