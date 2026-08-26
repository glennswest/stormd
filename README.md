# stormd

Container init system for scratch images. A single static binary that replaces shell, systemd, and cron inside minimal containers. SSH in, manage processes, tail logs, view a web dashboard — like a real Linux server, in 9 MB.

## Using stormdbase as a base image

The easiest way to use stormd is to build `FROM stormdbase` — a multi-arch scratch image (arm64 + amd64) that includes stormd, stormsh, and 63 busybox-style command symlinks pre-installed in `/bin`, `/usr/bin`, `/sbin`, and `/usr/sbin`. The container runtime picks the right architecture automatically.

**Registry:** `registry.gt.lo:5000/stormdbase:latest`

### Example: web service with liveness probe

```dockerfile
FROM registry.gt.lo:5000/stormdbase:latest
COPY my-app /app/server
COPY config.toml /etc/stormd/config.toml
EXPOSE 9080 8080 22
ENTRYPOINT ["/stormd"]
```

`config.toml`:

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
name = "web"
command = "/app/server"
args = ["--port", "8080"]
on_failure = "restart"
on_exit = "restart"
restart_delay_secs = 2

[process.liveness]
type = "http"
url = "http://localhost:8080/health"
interval_secs = 10
initial_delay_secs = 10
```

Build and run:

```bash
podman build --format docker -t my-service .
podman run -d --name my-service \
  -p 9080:9080 -p 8080:8080 -p 2222:22 \
  my-service

# Open dashboard
open http://localhost:9080/ui/

# SSH in — full shell with ls, cat, grep, curl, ping, etc.
ssh root@localhost -p 2222

# Get the instance cloud_id (auto-generated, also works as SSH password)
curl -s http://localhost:9080/api/v1/cloudid | jq -r .cloud_id

# SCP files into the container
scp -P 2222 mydata.tar.gz root@localhost:/data/

# SFTP session
sftp -P 2222 root@localhost
```

That's it. Your final image has a process supervisor, SSH server, web dashboard, REST API, liveness health checks, SCP/SFTP file transfer, and 63 Unix commands — all from a single static binary.

### Example: multi-process with per-process logging

Each `[[process]]` gets its own log stream, VT100 terminal, log archive, and liveness probe.

```dockerfile
FROM registry.gt.lo:5000/stormdbase:latest
COPY api-server /app/api
COPY worker /app/worker
COPY config.toml /etc/stormd/config.toml
EXPOSE 9080 8080 22
ENTRYPOINT ["/stormd"]
```

```toml
[general]
name = "my-stack"
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
name = "api"
command = "/app/api"
args = ["--port", "8080"]
on_failure = "restart"
on_exit = "restart"

[process.liveness]
type = "http"
url = "http://localhost:8080/health"
interval_secs = 10

[[process]]
name = "worker"
command = "/app/worker"
args = ["--concurrency", "4"]
on_failure = "restart"
on_exit = "stop"
depends_on = ["api"]
```

Per-process logging:
- Separate log files: `/var/stormd/logs/api.log`, `/var/stormd/logs/worker.log`
- Separate VT100 terminals viewable in web UI or via `attach api` in SSH
- Separate archived runs on the log volume (per-process, per-run)
- Filterable in logs UI: `?process=api` or `?process=worker`

SSH shell usage:
```bash
ps                  # see both processes with status
logs api            # view api logs only
logs worker         # view worker logs only
logs -f             # follow all logs
logs -f api         # follow api logs only
restart worker      # restart just the worker
```

### Building stormdbase

```bash
# ARM64 (Apple Silicon, Raspberry Pi, MikroTik)
cargo build --release --target aarch64-unknown-linux-musl
podman build --format docker --platform linux/arm64 -t stormdbase-arm64 -f Containerfile .

# x86_64
cargo build --release --target x86_64-unknown-linux-musl
podman build --format docker --platform linux/amd64 -t stormdbase-amd64 -f Containerfile.x86_64 .

# Create multi-arch manifest and push
podman manifest create stormdbase:latest
podman manifest add stormdbase:latest localhost/stormdbase-arm64:latest --arch arm64
podman manifest add stormdbase:latest localhost/stormdbase-amd64:latest --arch amd64
podman manifest push --all --tls-verify=false stormdbase:latest registry.gt.lo:5000/stormdbase:latest
```

### Projects using stormdbase

| Project | Description | Image |
|---------|-------------|-------|
| **netwatch** | Network monitoring and topology mapping | `registry.gt.lo:5000/netwatch:edge` |
| **microdns** | DNS/DHCP server with REST API | `registry.gt.lo:5000/microdns:edge` |
| **miniminio** | Minimal MinIO S3 gateway | `registry.gt.lo:5000/miniminio:edge` |
| **rust4git** | Git web interface | `registry.gt.lo:5000/rust4git:edge` |
| **mkube** | Container orchestrator | `registry.gt.lo:5000/mkube:edge` |

## What it does

- **Process supervisor** — launches and monitors one or more binaries with configurable restart policies
- **Web dashboard** — browser-based process management, memory charts, mount usage, restart history at `/ui/`
- **Plugin UI** — managed processes add custom tabs to the web UI via `[process.ui]` config, with reverse proxy and style guide
- **SSH server** — built-in SSH with a bash-like management shell (process control, log tailing, tab completion)
- **VT100 terminals** — per-process terminal emulation, viewable via SSH, WebSocket, or web UI
- **Structured logging** — severity detection shared with the fleet ([stormcast](https://github.com/glennswest/stormcast)), a rotated file per process on the log volume, RFC 5424 multicast to the fleet group, broadcast streams to follow
- **Log archival** — a run's file is named after the run on exit, failed or exited, and the run history is browsable in the UI. Old runs are pruned so a crash loop cannot fill the volume with the record of what went wrong.
- **Stdio capture** — captures stdout/stderr with automatic severity detection (PANIC/FATAL/ERROR/WARN)
- **Cron scheduler** — run commands on cron schedules
- **OCI image updater** — automatic image updates with blue/green rootfs pivot via stormpull
- **REST API** — full control plane for status, process management, logs, terminals, plugins, and debug
- **Graceful shutdown** — `POST /api/v1/shutdown` stops all processes and exits with optional exit code
- **Event system** — push events to NATS or webhooks when processes start/stop/crash
- **Liveness probes** — HTTP and TCP health checks with automatic restart on failure (SIGUSR1 grace, then SIGKILL)
- **Busybox commands** — 63 built-in Unix commands (ls, cat, grep, curl, ping, etc.) via argv[0] symlinks
- **Cloud ID** — per-instance unique identifier usable as SSH password; set via config, env var, or auto-generated UUID
- **CloudID SSH key auth** — fetches SSH public keys from CloudID metadata service (169.254.169.254) for passwordless login; 30s auto-refresh
- **SFTP/SCP** — built-in SFTP subsystem enables `scp` and `sftp` file transfers into and out of containers
- **Docker HEALTHCHECK** — `stormd --healthcheck` probes the running instance for use in scratch containers
- **PID 1** — proper zombie reaping, signal handling, and network sysctl init for scratch containers

## Workspace structure

```
stormd/
  Cargo.toml                   # workspace root
  crates/
    stormd/                    # main binary — init, supervisor, API, SSH server, web UI
    stormlog/                  # library — VT100, rotated files, multicast emit, streams
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

# Logs. Point log_dir at a volume and they outlive the container.
[stormlog.file]
log_dir = "/var/stormd/logs"

[[process]]
name = "api-server"
command = "/app/server"
args = ["--port", "8080"]
on_failure = "restart"
on_exit = "restart"

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
COPY server /app/server
COPY worker /app/worker
COPY cleanup /app/cleanup
COPY config.toml /etc/stormd/config.toml
VOLUME /var/stormd/logs
EXPOSE 9080 8080 22
ENTRYPOINT ["/stormd"]
```

## SSH shell commands

```
ps              — list supervised processes (colored status, liveness column)
start <name>    — start a process
stop <name>     — stop a process
restart <name>  — restart a process
attach <name>   — attach to process VT100 terminal
logs [name]     — show recent logs
logs -f [name]  — follow logs realtime
grep <pattern>  — search logs
liveness [name] — show liveness probe status and config
cron            — list cron jobs
status          — full system status (includes liveness health summary)
uptime          — container uptime
env             — environment variables
whoami          — current user (root)
hostname        — container name
df              — storage usage
free            — memory info
dmesg           — query all process logs from stormlog
systemctl       — systemd emulation (start/stop/restart/status/list-units)
help            — list commands
exit            — close SSH session
```

Shell features: tab completion, command history, colorized output, piping (`logs | grep error`), redirection (`cmd > file`, `cmd >> file`).

## Web UI

The web UI is a Svelte SPA embedded in the stormd binary (built from `web/`,
~24 KB gzipped, no node at runtime), served at `/ui/`.

| Page | Route | Description |
|------|-------|-------------|
| Dashboard | `/ui/#/` | Every component of the system as a live card — health, one-line detail, headline metrics, actions — plus the memory chart |
| Terminal | `/ui/#/terminal` | Live VT100 terminal output per process |
| Logs | `/ui/#/logs` | Log viewer with severity/stream filters, search, run selector for crash history |
| Process | `/ui/#/process/{name}` | One process: its card plus its live terminal |
| Plugin | `/ui/#/ext/{name}` | Custom app UI served via reverse proxy with stormd nav chrome |

The pre-SPA URLs (`/ui/terminal`, `/ui/logs`, `/ui/ext/{name}`) redirect to
their hash routes, so old bookmarks keep working.

The dashboard renders the component-summary feed (`/api/v1/components`, pushed
live over `/ws/components`) generically: a subsystem that reports a summary
appears as a card with no frontend changes, and stormsh's dashboard renders
the same feed as TUI tiles. The contract types live in the shared
[stormview](https://github.com/glennswest/stormview) crate. To develop the UI
against a running stormd: `cd web && STORMD_URL=http://host:9080 npm run dev`;
`npm run build` writes `web/dist`, which is committed and embedded at the next
cargo build.

**Themes** — six built in (Storm, Midnight, Nord, Solar, Phosphor, Light),
picked from the nav bar and remembered per browser. A theme is one block of
CSS token overrides in `web/src/app.css` — colors, ANSI palette for rendered
output, chart colors — so adding a theme is adding a block.

**Grid view** — the dashboard toggles between cards and a relational grid.
Components carry typed relations (`has_one`, `has_many`, `belongs_to`) between
ids in the feed; the grid nests child grids along `has_many`/`has_one` edges
(system → processes → their update images), rows multi-select for bulk
start/stop/restart, and `has_many` edges render as "select from a
relationship" pickers. A ⊞ on any card opens `#/grid` rooted at that
component or one of its relationships.

**The UI system is the stormview npm package** — themes, `DataGrid`,
`ComponentCard`, `ComponentGrid`, `RelationPicker`, `HealthDot`, and the
shared helpers all live in the same repo as the contract
([stormview](https://github.com/glennswest/stormview), installed from git),
so stormdrive and stormconsole consume the identical UI system. stormd's
`web/` keeps only the app: routing, stores, auth, and views. After pushing a
stormview change, run `npm update stormview` here to pick up the new commit.

**Login** — off by default. Setting `[api] password` (interactive) and/or
`[api] auth_token` (machine bearer token) turns authentication on: the UI
shows a login screen, sessions are HttpOnly cookies (in-memory, 24h), and
every endpoint except `/api/v1/health`, `/metrics`, the auth endpoints and
the static assets requires a session or `Authorization: Bearer <token>`. The
plugin proxy is protected. stormsh passes the token with `-t`/`--token` or
`STORMD_TOKEN`.

### Plugin UI

Any managed process can add its own tab to the stormd web UI without recompiling stormd. Add a `[process.ui]` section to your process config:

```toml
[[process]]
name = "myapp"
command = "/app/myapp"
args = ["--port", "3000"]

[process.ui]
label = "My App"
proxy = "http://127.0.0.1:3000"
# Optional: the plugin's own component summary, merged into its dashboard
# card (JSON with any of health/detail/metrics; best-effort, 400ms timeout)
summary = "http://127.0.0.1:3000/api/summary"
```

A `summary` endpoint returns JSON like:

```json
{
  "health": "ok",
  "detail": "serving 42 clients",
  "metrics": [
    { "label": "clients", "value": "42", "tone": "accent" },
    { "label": "queue", "value": "0", "tone": "muted" }
  ]
}
```

Every field is optional — `health` and `detail` replace the supervisor's
process-level view of the plugin's card, `metrics` append after it. Tones are
`ok`, `warn`, `error`, `muted`, `accent`.

This adds a "My App" tab to the nav bar. When clicked, stormd serves a page with its nav chrome and an iframe. The iframe content is reverse-proxied through stormd at `/ui/proxy/myapp/`, so:

- Same-origin — no CORS issues, cookies and fetch work naturally
- The app doesn't need to be directly reachable from the browser
- All HTTP methods are forwarded (GET, POST, PUT, DELETE, PATCH)
- Content-type headers are preserved (HTML, CSS, JS, JSON, images all work)

The proxy path structure:
```
/ui/ext/myapp          → stormd nav + iframe (what the user sees)
/ui/proxy/myapp/       → proxied to http://127.0.0.1:3000/
/ui/proxy/myapp/foo    → proxied to http://127.0.0.1:3000/foo
/ui/proxy/myapp/api/x  → proxied to http://127.0.0.1:3000/api/x
```

#### Example: app with its own UI

```toml
[general]
name = "my-stack"

[[process]]
name = "api"
command = "/app/api"
args = ["--port", "8080"]

[[process]]
name = "admin"
command = "/app/admin-ui"
args = ["--port", "3001"]

[process.ui]
label = "Admin"
proxy = "http://127.0.0.1:3001"

[[process]]
name = "grafana"
command = "/app/grafana-server"
args = ["--homepath", "/app/grafana"]

[process.ui]
label = "Metrics"
proxy = "http://127.0.0.1:3002"
```

This gives you five tabs: Dashboard, Terminal, Logs, Admin, Metrics.

#### Style guide for plugin UIs

Plugin UIs render inside an iframe that fills the viewport below stormd's 48px nav bar. To match stormd's visual style:

**Colors (Dracula-inspired dark theme):**
```css
/* Background and text */
body { background: #0f0f1a; color: #e0e0e0; }

/* Accent colors */
--red:    #e94560;    /* errors, danger, brand */
--green:  #50fa7b;    /* success, running, healthy */
--yellow: #f1fa8c;    /* warnings, caution */
--cyan:   #8be9fd;    /* links, info, accents */
--pink:   #ff79c6;    /* highlights */
--purple: #6272a4;    /* muted accents */

/* Surfaces */
--surface:    #16192e;   /* cards, nav, panels */
--border:     #2a2d45;   /* borders, dividers */
--hover:      #1e2140;   /* hover backgrounds */
--active:     #2a2d50;   /* active/selected state */
--input-bg:   #1a1d32;   /* form input backgrounds */
```

**Typography:**
```css
/* System font stack */
font-family: -apple-system, 'Segoe UI', system-ui, sans-serif;

/* Monospace (for code, logs, data) */
font-family: 'SF Mono', 'Fira Code', 'Cascadia Code', monospace;
```

**Component patterns:**
```css
/* Cards */
.card {
    background: #16192e;
    border: 1px solid #2a2d45;
    border-radius: 8px;
    padding: 16px 20px;
}

/* Buttons */
button {
    background: #2a2d50;
    color: #e0e0e0;
    border: 1px solid #3a3d60;
    padding: 6px 14px;
    border-radius: 6px;
    font-size: 13px;
}

/* Status badges */
.badge {
    display: inline-block;
    padding: 2px 8px;
    border-radius: 10px;
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
}
.badge-green { background: #1a4a2a; color: #50fa7b; }
.badge-red   { background: #4a1a2a; color: #e94560; }

/* Tables */
th { font-size: 11px; font-weight: 600; text-transform: uppercase;
     letter-spacing: 0.5px; color: #666; }
td { font-size: 13px; border-bottom: 1px solid #1a1d32; }
tr:hover { background: #1a1d32; }

/* Form inputs */
input, select {
    background: #1a1d32;
    color: #e0e0e0;
    border: 1px solid #2a2d45;
    padding: 6px 12px;
    border-radius: 6px;
    font-size: 13px;
}
```

**Key dimensions:**
- stormd nav bar: 48px height (your iframe gets `calc(100vh - 48px)`)
- Card border-radius: 8px
- Button/input border-radius: 6px
- Badge border-radius: 10px
- Base font size: 13px
- Label font size: 11px, uppercase, letter-spacing 0.5px

Your app doesn't have to match stormd's style — it renders in its own iframe and can use any framework. The style guide is just for visual consistency if you want it.

## Configuration reference

```toml
[general]
name = "my-service"                    # container name (shown in UI nav)
log_dir = "/var/stormd/logs"           # log file directory
# cloud_id = "my-unique-id"           # unique instance ID (also accepted as SSH password)
                                       # auto-generated UUID if not set (env: STORM_CLOUD_ID)

[api]
bind = "0.0.0.0:9080"                 # REST API + web UI bind address
# password = "changeme"               # UI login password — setting this (or
                                       # auth_token) turns authentication on
# auth_token = "s3cret"               # machine credential: Authorization: Bearer <token>

[ssh]
enabled = true                         # enable built-in SSH server
bind = "0.0.0.0:22"                   # SSH bind address
password = "stormd"                    # SSH password (default: stormd)
# host_key = "/etc/stormd/host_key"   # SSH host key path (auto-generated if missing)
# owner = "my-namespace"              # CloudID owner tag — enables SSH public key auth
# cloudid_url = "http://169.254.169.254"  # CloudID metadata endpoint (default: magic IP)

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

[stormlog.mcast]
# group = "239.255.42.1:5514"          # the fleet group; "off" to stay quiet

[stormlog.terminal]
rows = 24                              # VT100 terminal rows
cols = 80                              # VT100 terminal columns
scrollback = 1000                      # scrollback buffer size

[stormlog.file]
max_size_bytes = 104857600             # 100 MiB per log file before rotation
max_files = 10                         # rotated generations to keep, per process
max_runs = 10                          # finished runs to keep, per process

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

# Plugin UI — add a custom tab to the stormd web UI
# [process.ui]
# label = "My App"                      # nav tab label
# proxy = "http://127.0.0.1:3000"       # URL to reverse-proxy
# summary = "http://127.0.0.1:3000/api/summary"  # optional: plugin's own card summary

# Liveness probe — restarts process if health check fails
[process.liveness]
type = "http"                          # http | tcp
url = "http://localhost:8080/health"   # HTTP probe URL
# port = 5432                          # TCP probe port (for type = "tcp")
interval_secs = 10                     # check interval (default: 10)
timeout_secs = 5                       # probe timeout (default: 5)
failure_threshold = 1                  # failures before restart (default: 1)
initial_delay_secs = 5                 # delay before first check (default: 5)

[[cron]]
name = "cleanup"                       # job name
schedule = "0 0 * * * *"              # cron expression (6-field with seconds)
command = "/app/cleanup"               # command to run
args = []                              # arguments
```

## Liveness probes

Liveness probes detect hung processes that haven't exited but have stopped responding. When the failure threshold is reached, stormd sends SIGUSR1 (grace period), waits 5 seconds, then SIGKILL if still running. The normal restart policy then takes over.

### HTTP probe

```toml
[[process]]
name = "web"
command = "/app/server"

[process.liveness]
type = "http"
url = "http://localhost:8080/health"
interval_secs = 10
failure_threshold = 3
initial_delay_secs = 15
```

### TCP probe

```toml
[[process]]
name = "postgres"
command = "/usr/bin/postgres"

[process.liveness]
type = "tcp"
port = 5432
interval_secs = 5
```

Check liveness status via SSH shell:

```
liveness              # show all probe status
liveness web          # show specific process probe
ps                    # LIVENESS column shows ok/FAIL(n)
status                # summary shows N/M healthy
systemctl status web  # includes liveness details
```

## Busybox commands

stormd acts as a busybox-style multi-call binary. When invoked via a symlink (e.g., `/bin/ls -> /stormd`), it runs the corresponding command directly. The `stormdbase` image has all 63 commands pre-linked in `/bin`, `/usr/bin`, `/sbin`, and `/usr/sbin`.

### Available commands

| Category | Commands |
|----------|----------|
| **File** | `ls`, `dir`, `cat`, `head`, `tail`, `cp`, `mv`, `rm`, `mkdir`, `touch`, `chmod`, `chown`, `find`, `ln`, `stat`, `pwd`, `wc`, `du`, `readlink`, `file`, `sha256sum`, `md5sum`, `tee` |
| **Network** | `ifconfig`, `ip`, `ping`, `curl`, `wget`, `netstat`, `ss`, `nslookup`, `dig`, `hostname`, `route` |
| **System** | `mount`, `df`, `free`, `uname`, `date`, `id`, `kill`, `printenv`, `export`, `unset`, `sleep`, `echo`, `env`, `whoami`, `which`, `type`, `lsof`, `true`, `false`, `clear` |
| **Text** | `sort`, `uniq`, `cut`, `tr`, `sed`, `rev`, `base64`, `xxd`, `grep` |

### Install symlinks manually

```bash
# Install all commands to /bin
stormd --install /bin

# List all available commands
stormd --list-commands
```

Piping and redirection work between commands:

```bash
ls -la /app | grep server
cat /etc/stormd/config.toml | grep process
curl http://localhost:8080/health > /tmp/health.txt
```

## REST API

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/v1/cloudid` | Instance cloud ID and container name |
| GET | `/api/v1/health` | Health check |
| GET | `/api/v1/status` | Full status (processes, cron, stats) |
| GET | `/api/v1/components` | Component summaries — every part of the system in one uniform shape (id, kind, label, health, detail, metrics, actions, relations); live push on `/ws/components` |
| POST | `/api/v1/auth/login` | Start a session (`{"password": "..."}`) — sets the session cookie |
| POST | `/api/v1/auth/logout` | End the session |
| GET | `/api/v1/auth/session` | `{required, authenticated}` — whether login is needed/held |
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
| GET | `/api/v1/logs/stored` | Query what is on the log volume (`?run_id=X`) |
| POST | `/api/v1/logs/ingest` | Structured log ingestion |
| GET | `/api/v1/mounts` | Disk/mount usage |
| GET | `/api/v1/memory/history` | Memory RSS/VMS history samples |
| GET | `/api/v1/cron` | List cron jobs with status |
| GET | `/api/v1/updates` | List OCI image update status |
| POST | `/api/v1/updates/{name}/trigger` | Trigger image update check |
| POST | `/api/v1/backup` | Trigger manual log backup |
| GET | `/api/v1/plugins` | List registered UI plugins |
| POST | `/api/v1/shutdown` | Graceful shutdown (optional `exitCode` in body) |
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
- `process_started`, `process_stopped`, `process_crashed`, `process_restarting`, `liveness_check_failed`
- `update_check_started`, `update_available`, `update_pulling`, `update_pivoting`, `update_completed`, `update_failed`
- `cron_executed`, `cron_failed`
- `backup_started`, `backup_completed`, `backup_failed`

## Cloud ID

Each stormd instance has a unique cloud ID that can be used as an SSH password. This provides per-instance credentials for fleet management without sharing a single password.

**Resolution order:**
1. Config file: `[general] cloud_id = "my-id"`
2. Environment variable: `STORM_CLOUD_ID=my-id`
3. Persisted file: `{log_dir}/.cloudid` (survives restarts if log_dir is on a volume)
4. Auto-generated UUID v4 (persisted to `{log_dir}/.cloudid`)

**Usage:**
```bash
# Retrieve cloud_id via API
curl http://localhost:9080/api/v1/cloudid

# SSH using cloud_id as password
ssh root@container-host -p 22
# enter the cloud_id as the password

# SCP files into the container
scp -P 22 myfile.tar.gz root@container-host:/data/

# SFTP session
sftp -P 22 root@container-host
sftp> put localfile.txt /app/
sftp> get /var/stormd/logs/api.log
```

The cloud_id is accepted alongside the configured SSH password — both work.

## CloudID SSH Key Auth

When `owner` is set in `[ssh]`, stormd fetches SSH public keys from the CloudID metadata service and accepts public key authentication. This eliminates password prompts and centralizes key management across all stormd containers.

CloudID resolves which keys to serve based on the requesting container's IP address and namespace owner annotation (`vkube.io/owner`). The magic IP `169.254.169.254` is routed to CloudID via DHCP option 121 on all data networks.

**Config:**
```toml
[ssh]
enabled = true
bind = "0.0.0.0:22"
owner = "my-namespace"                       # activates CloudID key fetching
cloudid_url = "http://169.254.169.254"       # default — uses magic metadata IP
```

**How it works:**
1. On startup, stormd fetches authorized SSH keys from `{cloudid_url}/latest/meta-data/public-keys/`
2. Keys are refreshed every 30 seconds so changes propagate without restart
3. SSH clients can authenticate with their SSH key — no password needed
4. Password auth (configured password + cloud_id) remains available as fallback
5. If CloudID is unreachable, stormd starts with an empty key store and retries

**Usage:**
```bash
# SSH in with your key (no password prompt)
ssh root@container-host -p 22

# SCP with key auth
scp -P 22 myfile.tar.gz root@container-host:/data/
```

**Example configs:**
```toml
# mkube container
[ssh]
enabled = true
owner = "mkube"

# app in user namespace
[ssh]
enabled = true
owner = "gwest"
```

## Version

0.3.0
