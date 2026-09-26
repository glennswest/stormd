# stormd

A container init for scratch images: one static binary that is PID 1,
supervises one or more processes, keeps their logs, and answers questions
about them over a REST API, a web console, SSH and a TUI client.

In stormcos, stormd is PID 1 of every supervised component container —
fastetcd, the rustkube control plane, the kubelet, stormdrive, stormstorage,
stormconsole, cadvisor, stormlb and the rest (see [How it ships](#how-it-ships)).

This README is written from the code at v0.7.0. Where something is parsed but
does nothing, it says so.

A 12-slide overview is in [docs/presentation.md](docs/presentation.md) (Marp:
`npx @marp-team/marp-cli docs/presentation.md`).

## What it does today

- **Supervises processes** — start order with `depends_on` and ready probes,
  restart policies for crashes (`on_failure`) and clean exits (`on_exit`), an
  escalating restart delay capped at 30 s, a restart budget per window, and
  exit codes a process can declare not worth retrying (`no_restart_exit_codes`).
- **Liveness probes** — HTTP or TCP; on failure, SIGUSR1, 5 s grace, then
  SIGKILL, and the restart policy takes over.
- **Fills in node values** — `${NODE_IP}` and `${NODE_NAME}` in a process's
  `args` and `env` values are expanded each time it is spawned.
- **Logs** — stdout/stderr per process to a rotated file on the log volume,
  each run's file kept (and pruned) when it exits, every line on the fleet's
  multicast syslog group (the [stormcast](https://github.com/glennswest/stormcast)
  wire), a VT100 screen per process, and live streams to follow.
- **Events** — lifecycle events written to the log always, and optionally
  POSTed to a webhook.
- **REST API + WebSockets + Prometheus `/metrics`** on one port (default 9080).
- **Web console** — a Svelte SPA embedded in the binary at `/ui/`, rendered
  from the same component feed as the TUI; plugin tabs for supervised
  processes that have their own UI.
- **SSH server** — a management shell (process control, logs, attach, 60-odd
  file/network/system commands, pipes and redirection) and an SFTP subsystem.
- **Busybox-style multi-call binary** — 63 commands through `argv[0]` symlinks,
  so a scratch container has `ls`, `cat`, `curl`, `ping`, … .
- **Cron** — 6-field (seconds-first) schedules.
- **Log backup** — tar(.gz) the log directory and POST it somewhere when the
  container fails, or on demand.
- **OCI image updater** — processes with an `image` are pulled, unpacked into a
  rootfs directory, and swapped when the registry digest changes.
- **PID 1 duties** — reaps zombies, handles SIGTERM/SIGINT, writes a few
  network sysctls, and has a `--healthcheck` mode for Docker `HEALTHCHECK`.

## Workspace

```
crates/stormd/    the init/supervisor daemon (binary + lib)
  src/main.rs         startup, CLI, PID 1 duties, shutdown
  src/config.rs       every config key and its default
  src/supervisor.rs   process lifecycle, restart policy, probes
  src/nodevars.rs     ${NODE_IP} / ${NODE_NAME}
  src/api.rs          REST router, /metrics, plugin proxy
  src/auth.rs         login, sessions, bearer token
  src/components.rs   component-summary feed (both dashboards)
  src/ws.rs           WebSocket console / logs / components
  src/web.rs          embedded SPA
  src/ssh.rs sftp.rs  SSH server, SFTP subsystem
  src/shell/          SSH shell and busybox applets
  src/cron.rs events.rs backup.rs updater.rs cloudid.rs stats.rs debug.rs
crates/stormlog/  logging: rotated files, multicast emit, VT100, streams
crates/stormsh/   TUI client (ratatui)
test/             stormd-test: the test container (short/medium/long suites)
web/              Svelte 5 SPA source; web/dist is the built output (committed)
config/           example.toml — every key, parsed by a unit test
docs/             plugin UI guide, design notes
vendor/           vendored russh-sftp
```

Versions: stormd 0.7.3, stormsh 0.4.0, stormlog 0.3.0 (each crate's
`Cargo.toml`).

## Building

**Builds run on the build box, never on this VM and never as root.** Push
first, then from the checkout:

```bash
sc-build                         # cargo build && cargo test, on dev.g8.lo
sc-build 'cargo test -p stormd'  # any command
```

`sc-build` fetches the pushed commit onto `dev.g8.lo` as an unprivileged build
user, builds in a scratch directory and deletes it. A failure files a
`build-failure` issue here. Uncommitted changes are refused — it builds what
is on GitHub. Linux only matters: the PID 1, signal and `/proc` code is
`cfg(target_os = "linux")`, and a macOS build skips it.

Release binaries are static musl (`x86_64-unknown-linux-musl`,
`aarch64-unknown-linux-musl`, `armv7-unknown-linux-musleabihf`; linkers in
`.cargo/config.toml`). The release profile strips, uses LTO and
`panic = "abort"`.

**The web UI** is built separately and committed: `cd web && npm install &&
npm run build` writes `web/dist`, which `rust-embed` compiles into the stormd
binary, so a cargo-only build needs no node. The UI system (themes,
`DataGrid`, `ComponentCard`, …) is the
[stormview](https://github.com/glennswest/stormview) npm package, pinned in
`web/package-lock.json`; `npm update stormview` picks up a new commit. The
Rust contract types come from the same repo as a git dependency. To develop
against a running stormd: `cd web && STORMD_URL=http://host:9080 npm run dev`.

Sibling dependencies are git dependencies pinned in `Cargo.lock`: stormcast
(log wire), stormview (UI contract), stormpull (from the stormbase repo, for
the updater). A fix in one of them does not arrive here until `cargo update -p
<name>` and a commit of the lock file.

## Tests

Unit tests run with `cargo test` (so in every `sc-build`), including
`config/example.toml` being parsed and validated. The test container's crate
is a workspace member but not a default one, so a golden's release build never
compiles it: `sc-build 'cargo build --workspace && cargo test --workspace'`
covers it too.

**The test container**, `stormd-test-<suite>`, follows stormcentral's
[test standard](https://github.com/glennswest/stormcentral/blob/main/docs/test-standard.md):
built from `test/`, run by stormcentral as a Job in the run's own namespace
(`test/stormd-test.yaml`), one JSON object per test on stdout and in
`/results/results.jsonl`, exit 0 (all passed), 1 (a test failed) or 2 (could
not run).

It runs **the stormd of the commit under test** (`/stormd` in the image) as
its child, on configs it writes, and checks it through the REST API; the
processes stormd supervises are the test binary itself (`/test helper …`),
since the image is `FROM scratch`. So it needs no hardware (`requires: []`),
no cluster API and no ServiceAccount token, and everything it makes lives and
dies in the pod.

| suite | budget | covers |
|---|---|---|
| `short` | < 2 min | API up; a dependent waits for a tcp ready probe and for a one-shot to finish; a crash is restarted; stdout and stderr reach the logs API; SIGTERM exits 0 with no process left behind; the node's own stormds (ports 9081–9085) answer `/api/v1/health` — a skip where none do |
| `medium` | < 30 min | a failed one-shot holds its dependents, and SIGTERM still stops stormd; `no_restart_exit_codes` hold and fail; `on_failure = "fail"`; `max_restarts`; `on_exit = "restart"`; liveness restarts; API stop/start/restart and shutdown with an exit code; bearer-token auth; `/metrics`; the component feed; cron; a config that does not parse exits 1 |
| `long` | the night window | waves of processes sized from the pod's own CPU, memory and pid limits (mostly long-running, some crash-once, one-shots with dependents), started, settled and stopped with SIGTERM; one resident stormd has its processes restarted through the API every wave. Per wave: settle time, stop time, leftover processes, the resident's RSS and fds. A wave twice as slow as the first of its size, a leftover, or growing residue fails |

Build it on the build box (stormd needs `stormpull` over `ssh://`, so the
binaries are built by cargo there, not inside a container build):

```bash
test/build.sh short                   # static musl binaries → podman build stormd-test-short
STAGE_ONLY=1 test/build.sh            # just stage the context in test/.stage/
```

Run it by hand against a cargo build — it finds `stormd` next to itself, or
`STORMD_BIN`; scratch goes to a temp directory when there is no `/results`:

```bash
STORM_SUITE=short target/debug/stormd-test
STORM_SUITE=long STORM_TIMEOUT=600 target/debug/stormd-test
```

## How it ships

stormd is **not a golden of its own**. It is `/stormd` inside every
*stormdbase* golden — the base each supervised component golden is built on.
stormcentral's component registry lists it as `kind = "special"`, recipe "not a
golden: /stormd in every stormdbase golden".

The authority for how goldens are built is
[stormcos `docs/goldens.md`](https://github.com/glennswest/stormcos/blob/main/docs/goldens.md).
In short, stormcos `deploy/build-goldens.sh` (and stormcentral's golden
builder, which mirrors it and pins the stormd commit per build):

- builds stormd static for musl from this repo;
- `stormdbase_stage`: `/stormd`, applet links in `/bin` and `/usr/bin`
  (relative targets, `../stormd`, because a golden is mounted as a clone and an
  absolute target only resolves when the root is `/`), `/etc/stormd`,
  `/var/log/stormd` as the log volume's mount point;
- adds the component binary and `/etc/stormd/config.toml`, appends log limits
  sized to the 64 MiB log volume (`max_size_bytes = 8388608`, `max_files = 3`,
  `max_runs = 5`), and seals a deterministic tar into the golden;
- the container's `argv` is `/stormd`, and it reads `/etc/stormd/config.toml`.

So **a commit here reaches a node only when a new golden (and release) is
built** that picks it up. After work is pushed and sc-build passes, request
it with `stormcentral component build <component> --url
http://stormcentral.g8.lo` for the component whose golden should carry it.

### stormd's API port on a node

Every stormd on a node shares the host network, so each has its own port:

| Container | stormd API |
|---|---|
| fastetcd | 9081 |
| rustkube-apiserver / controller-manager / scheduler | 9082 / 9083 / 9084 |
| rustkube-node (kubelet, kube-proxy) | 9085 |
| `kind = "service"` goldens (stormdrive, stormstorage, stormconsole, cadvisor, stormlb, …) | the service's port + 100 (stormdrive 9192, stormstorage 9193, stormconsole 9194) |

A service golden's config also sets `no_restart_exit_codes = [78]` and an HTTP
liveness probe on the service's health path.

### Standalone container image

`Containerfile` (arm64), `Containerfile.x86_64` and `Containerfile.armv7` build
a scratch *stormdbase* OCI image from a prebuilt musl binary — stormd, stormsh
and the applet links in `/bin`, `/usr/bin`, `/sbin`, `/usr/sbin` — with
`HEALTHCHECK /stormd --healthcheck` and `ENTRYPOINT ["/stormd"]`. Use it as
`FROM` for a container outside stormcos:

```dockerfile
FROM stormdbase
COPY my-app /app/server
COPY config.toml /etc/stormd/config.toml
ENTRYPOINT ["/stormd"]
```

## Running

```
stormd [--config PATH]           # default /etc/stormd/config.toml
stormd --healthcheck [--healthcheck-port 9080]
stormd --install DIR             # create applet symlinks in DIR, exit
stormd --list-commands           # print the applet names, exit
stormd --version
```

Invoked through a symlink whose name is one of the 63 applets, stormd runs
that command and exits instead (see [Busybox commands](#busybox-commands)).

Startup, in order: install applet symlinks into `/bin`, `/usr/bin`, `/sbin`,
`/usr/sbin` (skipping names that exist; errors ignored) → load and validate the
config (exit 1 on error) → resolve the cloud ID → start logging → start cron
and the updater → start processes → bind the API (exit 1 if it cannot) and the
SSH server → reap zombies and set sysctls (Linux).

It shuts down on SIGTERM, SIGINT, `POST /api/v1/shutdown`, or container
failure — the same whether it is PID 1 or an ordinary process under a
supervisor or a test harness. From that moment nothing new starts: the start
order stops where it is (including a process still waiting on a `depends_on`
that will never be satisfied), restarts stand down, and API starts are
refused. It stops every process (see below — SIGKILL) and waits up to 10 s
for them to go, flushes logs, runs the backup if the container failed and
`[backup] on_failure` is set, and exits with the API-requested code, else 1 if
the container failed, else 0. If shutdown has not finished 30 s after it
began, stormd exits 1 regardless. A test that starts stormd should still use
`timeout -k 5 N`, so a regression here cannot hang a build.

Logging goes to stderr as plain compact lines — no timestamp, no ANSI, no JSON
(the envelope that carries them already has those). `RUST_LOG` overrides the
default filter `info,stormd=debug`.

Network sysctls written at start (Linux, best-effort):
`net.ipv4.icmp_echo_ignore_all=0`, `conf.all.accept_local=1`, `ip_forward=1`,
`conf.all.arp_ignore=0`, `conf.all.arp_announce=0`,
`conf.all.accept_redirects=1`, `net.ipv6.conf.all.disable_ipv6=0`.

### Ports

| Port | What | Config |
|---|---|---|
| 9080/tcp | REST API, WebSockets, `/metrics`, web UI | `[api] bind` |
| 22/tcp | SSH + SFTP (only when `[ssh] enabled = true`; off by default) | `[ssh] bind` |
| → 239.255.42.1:5514/udp | log lines out, RFC 5424 syslog (send only) | `[stormlog.mcast] group` |

stormd also connects out to: the CloudID metadata service (SSH keys, when
`[ssh] owner` is set), a webhook and a backup URL (when configured), and
registries (updater).

## Configuration reference

TOML, from `crates/stormd/src/config.rs` and `crates/stormlog/src/types.rs`.
Unknown keys are ignored silently. `config/example.toml` shows every key and is
parsed by a unit test.

Validation at load: at least one `[[process]]` or `[[cron]]`; process names
unique; each process has `command` or `image`; every `depends_on` names a
process; `[events]` with `transport = "webhook"` needs `webhook_url`;
`[backup] enabled` needs `destination_url`.

### `[general]`

| Key | Default | |
|---|---|---|
| `name` | `"stormd"` | container name: UI, events, metrics `container` label |
| `log_dir` | `"/var/log/stormd"` | per-process log files and `.cloudid`; created at start. **Overrides `[stormlog.file] log_dir`** |
| `cloud_id` | — | see [Cloud ID](#cloud-id) |
| `theme` | — | default web UI theme id (below); a viewer's own pick wins |
| `pid_file` | `"/run/stormd.pid"` | **parsed, not used** — no PID file is written |

Theme ids (from stormview): `storm`, `one`, `gruvbox`, `catppuccin`, `rose`,
`midnight`, `nord`, `solar`, `phosphor` (dark); `light`, `frost`, `paper`
(light).

### `[api]`

| Key | Default | |
|---|---|---|
| `bind` | `"0.0.0.0:9080"` | REST API, WS, metrics and UI |
| `auth_token` | — | bearer token for any request; also the `admin` login password |
| `password` | — | legacy: user `admin` with this password |
| `[[api.users]]` | — | `name`, `password` — UI login users |
| `[api.hosts]` | — | `"host.name" = "/path"`: a request for `/` whose `Host:` matches is redirected there (default `/ui/`) |

Any of `auth_token`, `password` or a user turns authentication on — see
[Authentication](#authentication).

### `[[process]]`

| Key | Default | |
|---|---|---|
| `name` | required | unique |
| `command` | `""` | binary path; required unless `image` is set |
| `args` | `[]` | `${NODE_IP}` / `${NODE_NAME}` expanded at spawn |
| `env` | `{}` | added to stormd's own environment; values expanded like `args` |
| `working_dir` | stormd's | |
| `image` | — | OCI image; the process is then run by the updater (below), not at start |
| `on_failure` | `"restart"` | non-zero exit or signal: `restart` \| `fail` \| `ignore` |
| `on_exit` | `"restart"` | exit 0: `restart` \| `stop` |
| `restart_delay_secs` | `1` | base delay; doubles per restart in the window, capped at 30 s |
| `max_restarts` | `10` | per `restart_window_secs` |
| `restart_window_secs` | `3600` | |
| `no_restart_exit_codes` | `[]` | exit codes that mean "a restart will not fix this" |
| `on_no_restart` | `"hold"` | `hold` \| `fail` |
| `depends_on` | `[]` | names of processes to wait for |
| `startup_delay_secs` | `0` | sleep before the first spawn |
| `ready_probe` | — | inline table, below |
| `[process.liveness]` | — | below |
| `[process.ui]` | — | plugin tab, below |
| `capture_stdout`, `capture_stderr` | `true` | **parsed, not used** — both are always captured |

**`ready_probe`** — `{ type = "http", url = "...", interval_secs = N }`,
`{ type = "tcp", port = N, interval_secs = N }` or
`{ type = "exec", command = "bin arg ...", interval_secs = N }`
(`interval_secs` is required). Polled in the background after each spawn,
5 s timeout per attempt; HTTP passes on 2xx/3xx (certificates not verified),
TCP connects to `127.0.0.1:port`, exec passes on exit 0. It gates dependents
only.

**`[process.liveness]`**

| Key | Default | |
|---|---|---|
| `type` | required | `http` (with `url`) \| `tcp` (with `port`, on 127.0.0.1) |
| `interval_secs` | `10` | |
| `timeout_secs` | `5` | |
| `failure_threshold` | `1` | consecutive failures before acting |
| `initial_delay_secs` | `5` | after each spawn |

HTTP passes on 2xx or 3xx and does not verify certificates (the supervisor
already knows what it started; a self-signed apiserver was otherwise killed
every 25 s).

**`[process.ui]`** — `label` (required), `proxy` (required, URL), `host`
(Host-based route to this plugin), `summary` (URL of the plugin's own card
summary). See [docs/plugin-ui.md](docs/plugin-ui.md).

### `[[cron]]`

| Key | Default | |
|---|---|---|
| `name` | required | |
| `schedule` | required | 6 fields, seconds first: `sec min hour dom month dow` (the `cron` crate) |
| `command` | required | |
| `args` | `[]` | |
| `env` | `{}` | |
| `timeout_secs` | `300` | after this the run is recorded as failed; **the job is not killed** |
| `capture_output` | `true` | once the job ends, its stdout/stderr are logged as process `cron.<name>` (stderr as warnings) |

### `[events]`

| Key | Default | |
|---|---|---|
| `enabled` | `false` | |
| `transport` | `"none"` | `none` \| `webhook` |
| `webhook_url` | — | each event is POSTed as JSON |
| `webhook_headers` | `{}` | |

Events are written to the log (process `event`) whether or not this is
enabled. See [Events](#events-1).

### `[backup]`

| Key | Default | |
|---|---|---|
| `enabled` | `false` | |
| `on_failure` | `true` | back up when the container fails |
| `destination_url` | — | required when enabled; the archive is POSTed here |
| `headers` | `{}` | added to the POST |
| `compress` | `true` | `application/gzip` tar.gz, else `application/x-tar` |

The archive is the whole `log_dir` under `logs/`. `POST /api/v1/backup` runs
it on demand.

### `[updater]`

| Key | Default | |
|---|---|---|
| `enabled` | `false` | |
| `poll_interval_secs` | `60` | |
| `data_dir` | `"/data/images"` | blob store |
| `rootfs_dir` | `"/data/rootfs"` | `<name>`, `<name>.new`, `<name>.old` |
| `registry` | `"registry.gt.lo"` | **parsed, not used** — the registry comes from each `image` reference |

See [Image updater](#image-updater).

### `[ssh]`

| Key | Default | |
|---|---|---|
| `enabled` | `false` | |
| `bind` | `"0.0.0.0:22"` | |
| `host_key` | `"/etc/stormd/host_key"` | ed25519, generated and saved if missing |
| `password` | `"stormd"` | the cloud ID is also accepted |
| `owner` | — | when set, public-key auth from CloudID is on |
| `cloudid_url` | `"http://169.254.169.254"` | |
| `authorized_keys` | — | **parsed, not used** |

### `[debug]`

| Key | Default | |
|---|---|---|
| `enabled` | `false` | adds `GET /api/v1/debug/info` and `/api/v1/debug/config` |
| `allow_signal` | `false` | adds `POST /api/v1/debug/processes/{name}/signal` |
| `allow_stdin` | `false` | adds `POST /api/v1/debug/processes/{name}/stdin` |
| `dynamic_log_level` | `false` | **parsed, not used** |

### `[stormlog.file]`, `[stormlog.mcast]`, `[stormlog.terminal]`

| Key | Default | |
|---|---|---|
| `file.max_size_bytes` | `104857600` (100 MiB) | rotate `<process>.log` at this size |
| `file.max_files` | `10` | rotated generations kept per process |
| `file.max_runs` | `10` | finished runs kept per process |
| `file.log_dir` | — | **ignored**: always `[general] log_dir` |
| `mcast.group` | `239.255.42.1:5514` | `host:port`, or `"off"` |
| `mcast.host` | this machine's hostname | syslog HOSTNAME field (the node, not the container) |
| `terminal.rows` / `cols` | `24` / `80` | VT100 screen per process |
| `terminal.scrollback` | `1000` | lines |

### `[log]`

`max_size_bytes`, `max_files`, `timestamps`, `json_format` — **parsed, not
used.** Rotation is `[stormlog.file]`.

## Process supervision

Processes without `image` are started at boot **in config order**; each first
waits for its `depends_on` (polled every 250 ms), then `startup_delay_secs`,
then is spawned. A dependency is satisfied when it is running and its
`ready_probe` (if any) has passed. A one-shot (`on_exit = "stop"`) — a
migration, a cert-minting task — satisfies when it has **finished**: stopped
after exiting 0. Without a `ready_probe`, running is not enough, because a
process with no probe is "ready" the moment it is spawned, while it is still
doing the work its dependents wait for; a one-shot *with* a probe satisfies
on the probe as well. A one-shot that failed (left stopped by
`on_failure = "ignore"`, or failed) or was stopped by hand never satisfies:
its dependents stay held and one WARN line names the dependency.
A later process in the list waits behind an earlier one that is still waiting.

stdin, stdout and stderr are pipes: output goes to the log, stdin is reachable
through the debug API.

**Stopping is SIGKILL.** A stop or restart (API, shell, stormsh, UI) and
shutdown all kill the process outright — there is no SIGTERM and no grace
period today, so a process gets no chance to flush or deregister. A process
that must shut down cleanly has to be told another way first.

| Exit | Policy | Result |
|---|---|---|
| 0 | `on_exit = "restart"` | restarted after the delay; past `max_restarts` it is left **stopped** |
| 0 | `on_exit = "stop"` | stopped |
| non-zero / signal | `on_failure = "restart"` | restarted after the delay; past `max_restarts` the process is **failed** and so is the container |
| non-zero / signal | `on_failure = "fail"` | process failed, container failed |
| non-zero / signal | `on_failure = "ignore"` | stopped; container keeps running |
| code in `no_restart_exit_codes` | `on_no_restart = "hold"` | process **failed**, not restarted, not counted; container keeps running |
| code in `no_restart_exit_codes` | `on_no_restart = "fail"` | as above, and the container fails |

The delay before restart *n* within the window is `restart_delay_secs × 2^(n-1)`,
capped at 30 s — a process that can never start costs a restart every 30 s,
not every second. Each exit is handled on its own, so one process waiting out
its delay does not hold up another's exit, restart or state. `no_restart_exit_codes` is how a process says a restart
cannot help (sysexits 78 `EX_CONFIG`, 64 `EX_USAGE`; stormconsole exits 78): it
logs `process exited with a non-retryable code — not restarting` once, and the
`process_crashed` event carries `code` and `no_restart`. A clean exit is never
treated as one, and a death by signal has no code to match.

A failed container makes stormd shut down (checked every second) and exit 1.

**Liveness:** after `initial_delay_secs`, every `interval_secs`. When
`failure_threshold` consecutive probes fail stormd emits
`liveness_check_failed`, sends SIGUSR1, waits 5 s, and sends SIGKILL if the
process is still there; the exit then goes through the table above.

**`${NODE_IP}` and `${NODE_NAME}`** in `args` and `env` values (not `command`)
are replaced at every spawn. `NODE_IP` is the source address the routing table
picks for an off-node destination (no packet is sent); `NODE_NAME` is
`/proc/sys/kernel/hostname`. A name with no value is left as written.

## Logging

Every line of every process, and every event, goes three places:

1. **A file** — `{log_dir}/{process}.log`, rotated at `max_size_bytes` to
   `{process}.1.log` … `{process}.{max_files}.log`. When a run ends the file is
   renamed `{process}.{run_id}.{failed|exited}.log` and the next run starts a
   fresh one; the oldest runs past `max_runs` are deleted. Before the rename
   stormd waits (up to 5 s) for the output pipes to drain, so the line that
   explains a crash is in the file.
2. **The fleet's multicast group** — RFC 5424 syslog over UDP, framed by
   [stormcast](https://github.com/glennswest/stormcast) (shared with
   stormpump). Send only; collecting and searching a fleet's logs is
   mcastsyslog's job.
3. **Live streams** — the VT100 screen per process and a broadcast channel,
   followed by the web console, stormsh, `attach`, and `/ws/logs`.

**Severity** is stormcast's judgement from the first 120 characters of the
line: klog prefixes (`F`/`E`/`W`/`I`/`D` + `MMDD`), else the *leftmost* of
`PANIC`/`FATAL` → critical, `ERROR` → error, `WARN` → warning, `DEBUG`/`TRACE`
→ debug, `INFO` → info (case-insensitive, so `level=error` counts). A line with
no level is info on stdout and warning on stderr. A crash adds
`*** PROCESS CRASHED *** exit code N` (or `killed by signal`) at emergency.

`POST /api/v1/logs/ingest` (`{process, line, stream?, severity?}`) adds a line
from outside.

## Events

Kinds: `container_starting`, `container_stopping`, `container_failing`,
`process_started`, `process_stopped`, `process_crashed`, `process_restarting`,
`liveness_check_failed`, `cron_executed`, `cron_failed`, `backup_started`,
`backup_completed`, `backup_failed`, `update_check_started`,
`update_available`, `update_pulling`, `update_pivoting`, `update_completed`,
`update_failed`. (`process_ready` exists in the type and is never emitted.)

Each is logged as `event=<kind> process=<p> container=<c> key=value…` —
critical for `container_failing`; error for crashes and failed
cron/backup/update; warning for restarts, liveness failures, stops. With the
webhook on, the event is also POSTed as JSON:
`{id, timestamp, kind, process, container, detail}`.

## REST API

On `[api] bind`. With auth on, everything except the endpoints marked *open*
needs a session cookie or `Authorization: Bearer <auth_token>`.

| Method | Path | |
|---|---|---|
| GET | `/` | *open* — redirect by `Host:` (`[api.hosts]`, plugin `host`), else `/ui/` |
| GET | `/api/v1/health` | *open* — `{"status":"ok"}` |
| GET | `/metrics` | *open* — Prometheus text, below |
| GET | `/api/v1/status` | `container_failed`, stats, processes, cron jobs |
| GET | `/api/v1/stats` | uptime, memory, process counts |
| GET | `/api/v1/cloudid` | `{cloud_id, container_name}` |
| GET | `/api/v1/components` | the component-summary feed (below) |
| POST | `/api/v1/auth/login` | *open* — `{username, password}` → session cookie |
| POST | `/api/v1/auth/logout` | *open* |
| GET | `/api/v1/auth/session` | *open* — whether login is required/held, instance name, default theme |
| GET | `/api/v1/processes` | all process statuses |
| GET | `/api/v1/processes/{name}` | one |
| POST | `/api/v1/processes/{name}/start` \| `stop` \| `restart` | |
| GET | `/api/v1/logs` | lines from the log files (`?process=&tail=&search=`) |
| GET | `/api/v1/logs/{process}` | same, one process (`?tail=&search=`) |
| GET | `/api/v1/logs/{process}/runs` | finished runs |
| GET | `/api/v1/logs/files` | files in `log_dir` with sizes |
| GET | `/api/v1/logs/files/{filename}` | one file |
| GET | `/api/v1/logs/stored` | structured query (`?process=&stream=&search=&tail=&run_id=`) |
| POST | `/api/v1/logs/ingest` | add a line |
| GET | `/api/v1/terminal/{process}` | VT100 screen snapshot |
| GET | `/api/v1/cron` | jobs with last/next run, counts |
| GET | `/api/v1/updates` | updater state per image |
| GET | `/api/v1/updates/{name}` | one |
| POST | `/api/v1/updates/{name}/trigger` | check, pull and pivot now |
| POST | `/api/v1/backup` | run the backup now |
| GET | `/api/v1/mounts` | mount usage |
| GET | `/api/v1/memory/history` | RSS/VMS samples |
| GET | `/api/v1/plugins` | `[process.ui]` plugins |
| POST | `/api/v1/shutdown` | stop everything and exit; optional `{"exitCode": N}` |
| WS | `/ws/console/{process}` | live terminal |
| WS | `/ws/logs` | live lines (`?process=&severity=`) |
| WS | `/ws/components` | the component feed, a full snapshot every 2 s |
| ANY | `/ui/proxy/{name}/…` | reverse proxy to a plugin (auth required) |
| GET | `/ui/`, `/ui/…` | *open* — the embedded SPA |
| GET | `/api/v1/debug/info`, `/api/v1/debug/config` | with `[debug] enabled` |
| POST | `/api/v1/debug/processes/{name}/signal` | with `allow_signal` |
| POST | `/api/v1/debug/processes/{name}/stdin` | with `allow_stdin` |

**Health:** `GET /api/v1/health` answers `{"status":"ok"}` whenever the API
is up; it does not reflect process state (use `/api/v1/status` or
`/metrics`). `stormd --healthcheck` GETs it on `127.0.0.1:--healthcheck-port`
(default 9080 — pass the real port if `[api] bind` differs) with a 5 s
timeout and exits 0 or 1.

### Metrics

`GET /metrics`, Prometheus text 0.0.4, read at request time and kept nowhere.
Label `container` is `[general] name`; `process` is the supervised process.

| Metric | Type | |
|---|---|---|
| `stormd_up` | gauge | 1 |
| `process_start_time_seconds` | gauge | stormd's start time |
| `process_resident_memory_bytes`, `process_virtual_memory_bytes` | gauge | stormd's own memory |
| `stormd_uptime_seconds` | gauge | |
| `stormd_process_state{state}` | gauge | 1 for the current state of `running`, `stopped`, `failed`, `starting`, `restarting` |
| `stormd_process_restarts_total` | counter | |
| `stormd_process_crashes_total` | counter | non-zero exits |
| `stormd_process_liveness_failures_total` | counter | **current consecutive failures** — reset to 0 by a passing probe, so not monotonic despite the type |
| `stormd_process_uptime_seconds` | gauge | absent when not running |

### Component feed

`GET /api/v1/components` (and `/ws/components`) describes every part of the
instance in one shape — `id, kind, label, health, detail, metrics, actions,
relations` — with kinds `system`, `process`, `plugin`, `cron`, `storage`,
`logs`, `updater`, and typed relations (`has_one`, `has_many`, `belongs_to`).
Both the web dashboard and stormsh render it generically, so a new subsystem
appears in both by adding one summary source in `components.rs`. The contract
types are the [stormview](https://github.com/glennswest/stormview) crate.

## Authentication

Off unless `[api]` sets `auth_token`, `password` or `[[api.users]]`. Then:
the UI shows a login screen; a login sets an HttpOnly `stormd_session` cookie
(sessions are in memory, 24 h, gone on restart); `auth_token` works as a bearer
token on any request and as the password for `admin`; credentials are
compared in constant time. Open paths: `/`, `/metrics`, `/api/v1/health`,
`/api/v1/auth/*`, and `/ui/*` except `/ui/proxy/*`. stormsh passes the token
with `-t`/`--token` or `STORMD_TOKEN`.

## Web UI

The Svelte SPA at `/ui/` (hash routes):

| Route | |
|---|---|
| `#/` | dashboard: a card per component, or a relational grid (nested along `has_many`/`has_one`, multi-select bulk start/stop/restart, relation pickers); memory chart |
| `#/grid` | the grid rooted at a component (⊞ on a card) |
| `#/terminal` | live VT100 per process |
| `#/logs` | log viewer: severity/stream filters, search, run selector |
| `#/process/{name}` | one process: card and terminal |
| `#/ext/{name}` | a plugin's UI in an iframe |

Twelve themes (ids under [`[general]`](#general)), picked in the nav and
remembered per browser. The pre-SPA URLs `/ui/terminal`, `/ui/logs` and
`/ui/ext/{name}` redirect to their hash routes.

A supervised process with `[process.ui]` gets its own tab, reverse-proxied
same-origin at `/ui/proxy/{name}/`, and may feed its dashboard card through a
`summary` URL — see [docs/plugin-ui.md](docs/plugin-ui.md).

## stormsh

A ratatui client for a running stormd.

```
stormsh [-H HOST] [-p PORT] [-t TOKEN]    # defaults 127.0.0.1, 9080, $STORMD_TOKEN
```

Views: `1` dashboard (the component feed as tiles), `2` processes, `3`
terminal, `4` logs; `Tab` cycles. `↑/↓` or `j/k` select, `s`/`x`/`r`
start/stop/restart, `u` triggers an update (dashboard), `l` logs, `q`/`Esc`
quits.

## SSH

With `[ssh] enabled = true`. Any username; the password is `[ssh] password`
or the cloud ID. With `owner` set, public keys are fetched from
`{cloudid_url}/latest/meta-data/public-keys/` (index, then
`/{idx}/openssh-key`) at start and every 30 s, keeping the old set if a
refresh fails; the owner value itself only switches this on — it is not sent.
The requests speak IMDSv2: a token from `PUT /latest/api/token` (asked for
six hours, re-asked a minute before it expires, or once when a request with
it is answered 401) goes on every GET as `X-aws-ec2-metadata-token`, along
with `Metadata-Flavor: StormIMDS` — so every stormimds `security.mode`
works, and a service that issues no token is asked without one. A non-2xx
answer is a failed refresh, logged with its status once (and again only when
it changes, or when it recovers).

A session is an interactive shell (a PTY is expected); `ssh host command`
(exec requests) is not supported. The `sftp` subsystem is, so `sftp` and
OpenSSH's default (SFTP-based) `scp` work; legacy `scp -O` does not.

The shell, in addition to every applet below:

```
ps / top            processes with state and liveness
start|stop|restart <name>
attach <name>       the process's VT100 screen
logs [-f] [name]    recent / follow
dmesg [-f]          all processes
grep <pat> [file]   logs, or a file
liveness [name]     probe config and status
cron                jobs
status              everything, with a liveness summary
uptime
systemctl start|stop|restart|status|list-units [name]
xargs               runs the shell's own commands
help, exit
```

Tab completion (commands, process names, paths), history, `|` pipes, and
`>` / `>>` redirection.

## Busybox commands

`argv[0]` dispatch, 63 commands:

| | |
|---|---|
| File | `ls dir cat head tail cp mv rm mkdir touch chmod chown find ln stat pwd wc du readlink file sha256sum md5sum tee` |
| Network | `ifconfig ip ping curl wget netstat ss nslookup dig hostname route` |
| System | `mount df free uname date id kill printenv export unset sleep echo env whoami which type lsof true false clear` |
| Text | `sort uniq cut tr sed rev base64 xxd grep` |

`stormd --install DIR` links them all to the running binary; stormd also does
this at every start for `/bin`, `/usr/bin`, `/sbin` and `/usr/sbin`. Piped
stdin works (`ls /app | grep server`).

## Cloud ID

A per-instance identifier, usable as the SSH password and served at
`/api/v1/cloudid`. Resolved once at start, first match wins:

1. `[general] cloud_id`
2. `$STORM_CLOUD_ID`
3. `{log_dir}/.cloudid`
4. a new UUID v4, written to `{log_dir}/.cloudid`

## Image updater

With `[updater] enabled = true`, each `[[process]]` with `image` is owned by
the updater rather than started at boot. For each: if `rootfs_dir/<name>` does
not exist it pulls the image (stormpull, from the registry named in the
reference), unpacks it to `<name>.new`, and pivots — stop the process, move the
current rootfs to `<name>.old`, `<name>.new` into place, set the command to the
image entrypoint *inside* that directory (it is not chrooted), merge the
image's env under the config's, set the working directory, start it, and
delete `.old` in the background. Then every `poll_interval_secs` it HEADs the
manifest and repeats the pull and pivot when the digest changes.
`POST /api/v1/updates/{name}/trigger` does it now.

Two things the updater does not do today: it does not start an image process
whose rootfs already exists when stormd starts (it only waits for the next
digest change), and with the updater disabled an `image` process never runs.

## Development notes

- `web/dist` is committed; rebuild it when `web/` or stormview changes.
- New dashboard content: add a summary source in `components.rs`; no frontend
  change.
- Design notes and history: [docs/](docs/). Changes: [CHANGELOG.md](CHANGELOG.md).
