# stormd

A container init for scratch images: one static binary that is PID 1,
supervises one or more processes, keeps their logs, and answers questions
about them over a REST API, a web console, SSH and a TUI client.

In stormcos, stormd is PID 1 of every supervised component container —
fastetcd, the rustkube control plane, the kubelet, stormdrive, stormstorage,
stormconsole, cadvisor, stormlb and the rest (see [How it ships](#how-it-ships)).

This README is written from the code at v0.7.4 (refreshed 2026-09-27). Where something is parsed but
does nothing, or does something other than it says, it says so and names the issue.

A 12-slide overview is in [docs/presentation.md](docs/presentation.md) (Marp:
`npx @marp-team/marp-cli docs/presentation.md`).

## What it does today

- **Supervises processes** — start order with `depends_on` and ready probes,
  restart policies for crashes (`on_failure`) and clean exits (`on_exit`), an
  escalating restart delay capped at 30 s, a restart budget per window, and
  exit codes a process can declare not worth retrying (`no_restart_exit_codes`).
  A one-shot (`on_exit = "stop"`) satisfies its dependents once it has exited 0.
- **Liveness probes** — HTTP or TCP; on failure, SIGUSR1, 5 s grace, then
  SIGKILL, and the restart policy takes over.
- **Fills in node values** — `${NODE_IP}` and `${NODE_NAME}` in a process's
  `args` and `env` values are expanded each time it is spawned.
- **Node-overridable defaults** — `env_default` entries apply only when stormd
  did not inherit the key, so a node's env.d can override them.
- **Goldens** — a process can name stormblock goldens; stormd attaches each
  read-only and presents it (a filesystem golden mounted read-only, an image
  golden as a readable device node) before the process starts, and swaps one
  for another at runtime (#36).
- **Logs** — stdout/stderr per process to a rotated file on the log volume,
  each run's file kept (and pruned) when it exits, every line on the fleet's
  multicast syslog group (unlimited — #12) (the [stormcast](https://github.com/glennswest/stormcast)
  wire), a VT100 screen per process, and live streams to follow.
- **Events** — lifecycle events written to the log always, and optionally
  POSTed to a webhook.
- **REST API + WebSockets + Prometheus `/metrics`** on one port (default 9080).
- **Web console** — a Svelte SPA embedded in the binary at `/ui/`, rendered
  from the same component feed as the TUI; plugin tabs for supervised
  processes that have their own UI.
- **SSH server** — a management shell (process control, logs, attach, 60-odd
  file/network/system commands, pipes and redirection) and an SFTP subsystem;
  public keys from the CloudID metadata service, over IMDSv2.
- **Busybox-style multi-call binary** — 63 commands through `argv[0]` symlinks,
  so a scratch container has `ls`, `cat`, `curl`, `ping`, … .
- **Cron** — 6-field (seconds-first) schedules.
- **Log backup** — tar(.gz) the log directory and POST it somewhere when the
  container fails, or on demand.
- **OCI image updater** — processes with an `image` are pulled, unpacked into a
  rootfs directory, and swapped when the registry digest changes.
- **PID 1 duties** — reaps zombies, shuts down on SIGTERM/SIGINT (bounded:
  it exits within 30 s whatever stalls), writes a few network sysctls, and has
  a `--healthcheck` mode for Docker `HEALTHCHECK`.

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

Versions: stormd 0.7.4, stormsh 0.4.0, stormlog 0.3.0 (each crate's
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
[test standard](https://github.com/glennswest/stormcentral/blob/main/docs/test-standard.md)
as it stood on 2026-09-26. The standard and stormcentral's runner have since
moved to one image for all suites, built with the repo root as context and
started as `/test <suite>`; this container does not build that way yet
(#24):
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
| `medium` | < 30 min | a failed one-shot holds its dependents, and SIGTERM still stops stormd; `no_restart_exit_codes` hold and fail; `on_failure = "fail"`; `max_restarts`; `on_exit = "restart"`; liveness restarts; API stop/start/restart and shutdown with an exit code; bearer-token auth; `/metrics`; the component feed; cron; a config that does not parse exits 1; run as `ps` (not an applet), exits 127 and spawns nothing; `wait_for_files` holds the start until the file exists |
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

- builds stormd static for musl from this repo, at **main** (stormcentral's
  `component stage` fetches stormd at main; `component build` rebuilds a
  service golden whenever stormd has a new commit), with a bare
  `cargo build --release` — which is why the test crate is not a default
  workspace member;
- `stormdbase_stage`: `/stormd`, applet links in `/bin` and `/usr/bin` —
  exactly what `stormd --list-commands` prints, and no `ps` (stormcos#66)
  (relative targets, `../stormd`, because a golden is mounted as a clone and an
  absolute target only resolves when the root is `/`), `/etc/stormd`,
  `/var/log/stormd` as the mount point of the component's log volume — a
  copy-on-write clone of its `<component>-logs` golden (stormcos
  `docs/goldens.md`: service, `-data` and `-logs` goldens);
- adds the component binary and `/etc/stormd/config.toml`, appends log limits
  sized to the 64 MiB log volume (`max_size_bytes = 8388608`, `max_files = 3`,
  `max_runs = 5`), and seals a deterministic tar into the golden;
- the container's `argv` is `/stormd`, and it reads `/etc/stormd/config.toml`.

So **a commit here reaches a node when stormcos composes a release** and
rebuilds the goldens that carry stormd. stormd itself never requests a golden:
after work is pushed and sc-build passes, there is nothing to rebuild from
here.

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
stormd starts as init only when the basename of `argv[0]` is `stormd`, or a
renamed copy (`stormd-*`, `stormd.*`). Under any other name — `/bin/ps`
linked to stormd, say — it prints `stormd: ps: not a stormd applet (see
stormd --list-commands)` and exits 127 before reading a config or starting
anything (#11).

Startup, in order: install applet symlinks into `/bin`, `/usr/bin`, `/sbin`,
`/usr/sbin` (skipping names that exist; errors ignored) → load and validate the
config (exit 1 on error) → resolve the cloud ID → start logging → start cron
and the updater → start processes → start the SSH server (in the background;
a failed bind is logged and stormd carries on) → bind the API → reap zombies
and set sysctls (Linux). If the API cannot bind, stormd exits 1 at
once — **without stopping the processes the start order already spawned**,
which keep running unsupervised (#23).

It shuts down on SIGTERM, SIGINT, `POST /api/v1/shutdown`, or container
failure — the same whether it is PID 1 or an ordinary process under a
supervisor or a test harness. From that moment nothing new starts: the start
order stops where it is (including a process still waiting on a `depends_on`
that will never be satisfied), restarts stand down, and API starts are
refused. It stops every process in reverse dependency order (see below —
SIGTERM, `stop_timeout_secs`, then SIGKILL), waiting for each tier to go before
the next, flushes logs, runs the backup if the container failed and
`[backup] on_failure` is set, and exits with the API-requested code, else 1 if
the container failed, else 0. If shutdown has not finished by then, stormd
exits 1 regardless: the deadline is every tier's longest `stop_timeout_secs`
plus 2 s, summed, plus 20 s, and never less than 30 s. A test that starts stormd should still use
`timeout -k 5 N`, so a regression here cannot hang a build. An exit handled
after shutdown began is recorded as a stop, not a crash; under something that
signals the whole process group (`timeout`, a terminal's Ctrl-C) a child can
die before shutdown begins, and that exit is still logged as a crash with a
restart scheduled, which then stands down (#26).

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
`[backup] enabled` needs `destination_url`; a `[[process.golden]]` has a
unique name without `/`, exactly one of `golden`/`volume_id`, an absolute
`path` if any, and `owner` as `uid:gid`.

### `[general]`

| Key | Default | |
|---|---|---|
| `name` | `"stormd"` | container name: UI, events, metrics `container` label |
| `log_dir` | `"/var/log/stormd"` | per-process log files and `.cloudid`; created at start (exit 1 if it cannot be). **Overrides `[stormlog.file] log_dir`**. Put it on a volume: in a stormcos golden it is the component's `-logs` volume; a stormd run as a pod wants a PVC (on stormcos, the built-in stormblock PVC driver) — a container-local path is RAM on some hosts |
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
| `tls_cert_file`, `tls_key_file` | — | PEM certificate chain and key: serve the API over TLS (HTTP/1.1). Both or neither; re-read when either file changes |
| `client_ca_file` | — | PEM CA bundle (needs TLS): a client certificate that verifies against it authenticates the request; one that does not fails the handshake |
| `token_file` | — | file holding the bearer token (whitespace trimmed), re-read when it changes; an unreadable or empty file accepts no token |
| `auth_token` | — | bearer token for any request; also the `admin` login password |
| `password` | — | legacy: user `admin` with this password |
| `[[api.users]]` | — | `name`, `password` — UI login users |
| `[api.hosts]` | — | `"host.name" = "/path"`: a request for `/` whose `Host:` matches is redirected there (default `/ui/`) |

Any of `token_file`, `auth_token`, `client_ca_file`, `password` or a user
turns authentication on — see [Authentication](#authentication). With none of
them the API is anonymous, and stormd logs a warning at start saying so; with
auth on but no TLS it warns that credentials travel in the clear. On a
stormcos node every container's stormd answers on the node's address, so a
node's stormd should have all three: a stormcert serving pair, the node CA as
`client_ca_file`, and a `token_file` (#32). A missing, unreadable or
mismatched pair or CA stops stormd at start (exit 1), before any process is
spawned.

### `[[process]]`

| Key | Default | |
|---|---|---|
| `name` | required | unique |
| `command` | `""` | binary path; required unless `image` is set |
| `args` | `[]` | `${NODE_IP}` / `${NODE_NAME}` expanded at spawn |
| `env` | `{}` | added to stormd's own environment, over what it inherited; values expanded like `args` |
| `env_default` | `{}` | each entry set only when its key is **not** in stormd's own environment (an inherited empty value counts as set), so the node can override it — stormpump's `env.d/<spec>` reaches stormd as its environment. `env` wins over both. Values expanded like `args` |
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
| `wait_for_files` | `[]` | absolute paths that must all exist before the first start; `${NODE_IP}`/`${NODE_NAME}` expanded |
| `stop_timeout_secs` | `10` | on a stop: SIGTERM, wait this long, then SIGKILL; `0` = SIGKILL at once |
| `startup_delay_secs` | `0` | sleep before the first spawn |
| `ready_probe` | — | inline table, below |
| `[process.liveness]` | — | below |
| `[process.ui]` | — | plugin tab, below |
| `capture_stdout`, `capture_stderr` | `true` | **parsed, not used** — both are always captured |
| `[[process.golden]]` | — | goldens presented to the process, below and [Goldens](#goldens) |

**`ready_probe`** — `{ type = "http", url = "...", interval_secs = N }`,
`{ type = "tcp", port = N, interval_secs = N }` or
`{ type = "exec", command = "bin arg ...", interval_secs = N }`
(`interval_secs` is required). Polled in the background after each spawn,
5 s timeout per attempt; HTTP passes on 2xx/3xx (certificates not verified),
TCP connects to `127.0.0.1:port`, exec passes on exit 0. It gates dependents
only. A stormd applet always exits 0 (see [Busybox commands](#busybox-commands)),
so an exec probe built on one, such as `stat /file`, can never fail (#31).

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

Each job keeps its next fire time and runs when it comes (`GET /api/v1/cron`
shows it as `next_run`). Jobs run **one at a time**, inside the scheduler's
loop: a job that runs long delays every other job, for up to its
`timeout_secs`, and a fire time that passes meanwhile is run once, when the
loop comes round — not once per missed time.

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
| `mcast.group` | `239.255.42.1:5514` | `host:port`. **`"off"` (or `""`) does not silence today: it is replaced by the default group, so the lines still go out (#27)** |
| `mcast.host` | this machine's hostname | syslog HOSTNAME field (the node, not the container) |
| `terminal.rows` / `cols` | `24` / `80` | VT100 screen per process |
| `terminal.scrollback` | `1000` | lines |

### `[log]`

`max_size_bytes`, `max_files`, `timestamps`, `json_format` — **parsed, not
used.** Rotation is `[stormlog.file]`.

## Process supervision

Processes without `image` are started at boot **in config order**; each first
waits for its `depends_on` (polled every 250 ms, so up to a quarter second per
dependency — #25), then its `wait_for_files`, then `startup_delay_secs`,
then is spawned. `wait_for_files` holds the first start until every listed
file exists (polled every 250 ms; a dangling symlink counts as missing), with
one log line naming what is missing and one when it appears — a cert pair
another container mints, for example (#38). Waiting is not a restart: nothing
is counted toward `max_restarts` and there is no cool-off. Shutdown ends the
wait. A restart or an API start does not wait again. A dependency is satisfied when it is running and its
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

**Stopping is SIGTERM, then SIGKILL.** A stop or restart (API, shell, stormsh,
UI), the updater's pivot and shutdown all send the process SIGTERM, wait up to
its `stop_timeout_secs` (default 10) for it to exit, then SIGKILL it (#9). `0`
is SIGKILL at once. Only the process itself is signalled, not its children.
The exit code is recorded when it exits with one. A requested stop is never
restarted. A restart and the updater wait for the old run to be gone before
starting the new one. At shutdown, processes stop in reverse dependency order:
a process is stopped, and waited for, before anything it `depends_on` (the
apiserver before fastetcd).

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
its delay does not hold up another's exit, restart or state.

`no_restart_exit_codes` is how a process says a restart cannot help (sysexits 78 `EX_CONFIG`, 64 `EX_USAGE`; stormconsole exits 78): it
logs `process exited with a non-retryable code — not restarting` once, and the
`process_crashed` event carries `code` and `no_restart`. A clean exit is never
treated as one, and a death by signal has no code to match.

A failed container makes stormd shut down (checked every second) and exit 1.

**Liveness:** after `initial_delay_secs`, every `interval_secs`. When
`failure_threshold` consecutive probes fail stormd emits
`liveness_check_failed`, sends SIGUSR1, waits 5 s, and sends SIGKILL if the
process is still there; the exit then goes through the table above. Each
run has its own probe task and its own count: the task ends when that run
ends, and a restarted process starts at zero failures and waits its own
`initial_delay_secs` (before #45 a task from an earlier run could probe a
fresh restart at once with the old count, killing every restart).

**`${NODE_IP}` and `${NODE_NAME}`** in `args`, `env` and `env_default` values (not `command`)
are replaced at every spawn. `NODE_IP` is the source address the routing table
picks for an off-node destination (no packet is sent); `NODE_NAME` is
`/proc/sys/kernel/hostname`. A process is **never spawned** with one of these
two left unexpanded in an argument or in an environment value it would get
(#3). Instead stormd logs one ERROR, e.g. `process 'stormcert-init' needs
${NODE_IP}, and this node has no address on any interface (no route off the
node) — waiting, not starting it`, and waits, re-resolving every second. It
starts the process when the value appears (DHCP coming good), or gives up when
shutdown begins. This applies at the first start and before every restart.
Waiting counts nothing extra toward `max_restarts`. An API start is refused
with the same message. Any other `${…}`, such as a shell script's own
`${HOME}`, is passed through untouched.

## Logging

Every line of every process, and every event, goes three places:

1. **A file** — `{log_dir}/{process}.log`, rotated at `max_size_bytes` to
   `{process}.1.log` … `{process}.{max_files}.log`. When a run ends the file is
   renamed `{process}.{run_id}.{failed|exited}.log` and the next run starts a
   fresh one; the oldest runs past `max_runs` are deleted. Before the rename
   stormd waits (up to 5 s) for the output pipes to drain, so the line that
   explains a crash is in the file. The file is opened for every line; if the
   open fails (the directory removed, a mount gone, the disk full) each line
   logs its own `failed to open log file` ERROR on stormd's stderr, with no
   back-off and no attempt to recreate the directory (#1).
2. **The fleet's multicast group** — RFC 5424 syslog over UDP, framed by
   [stormcast](https://github.com/glennswest/stormcast) (shared with
   stormpump). Send only; collecting and searching a fleet's logs is
   mcastsyslog's job. **Every line is sent**: stormd does not use stormcast's
   limiter, so a process looping on one line, or printing thousands a second,
   puts each one on the group — no repeat collapse, no rate limit (#12;
   stormpump limits on the host, containers do not). The stormcast commit
   pinned in `Cargo.lock` (0.1.0, `9244121`) truncates a long line with
   `String::truncate` and **panics** when byte 8192 falls inside a multibyte
   character; the fix is in stormcast and arrives with `cargo update -p
   stormcast` (#28).
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

**stormd's own output** (stderr) carries stormd's log lines, not its
children's. A process that exits non-zero shows there only as
`WARN process exited with error process=<p> code=Some(N)`; the error it printed
is in its run file and on the multicast group, not in stormd's output. So a
host supervisor that keeps only stormd's output (stormpump, on a node console)
sees that it failed but not why (#29).

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

On `[api] bind`, over TLS when `tls_cert_file`/`tls_key_file` are set, plain
HTTP otherwise. With auth off (no credential configured) every route is open.
With auth on, everything except the endpoints marked *open* needs a verified
client certificate, a session cookie, or `Authorization: Bearer <token>`
(`auth_token` or the contents of `token_file`).

| Method | Path | |
|---|---|---|
| GET | `/` | *open* — redirect by `Host:` (`[api.hosts]`, plugin `host`), else `/ui/` |
| GET | `/api/v1/health`, `/healthz` | *open* — `{"status":"ok"}` |
| GET | `/metrics` | Prometheus text, below — behind auth like the rest |
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
| GET | `/api/v1/goldens` | goldens presented: process, name, golden, volume, content, device, path, size_bytes |
| PUT | `/api/v1/processes/{name}/goldens/{golden}` | `{"golden": "…"}` or `{"volume_id": "…"}` — swap it (see [Goldens](#goldens)) |
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

**Health:** `GET /api/v1/health` (and `/healthz`) answers `{"status":"ok"}` whenever the API
is up; it does not reflect process state (use `/api/v1/status` or
`/metrics`). `stormd --healthcheck` GETs it on `127.0.0.1:--healthcheck-port`
(default 9080 — pass the real port if `[api] bind` differs) over http, then
https if the port speaks TLS (certificate not checked: loopback, no
credential), with a 5 s timeout, and exits 0 or 1.

### Metrics

`GET /metrics`, Prometheus text 0.0.4, read at request time and kept nowhere.
With auth on it needs credentials like any other route (it names every
process): a scraper sends `Authorization: Bearer <token>` or presents a client
certificate, and with TLS uses `scheme: https` with the node CA.
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

Off unless `[api]` sets `token_file`, `auth_token`, `client_ca_file`,
`password` or `[[api.users]]`. Then a request is let in by any one of:

- a **client certificate** that verified against `client_ca_file` during the
  TLS handshake (a certificate from another CA fails the handshake; no
  certificate at all is fine — the other ways still apply);
- `Authorization: Bearer <token>`, where the token is `auth_token` or the
  current contents of `token_file` (re-read when the file changes, so a
  rotated token works without a restart); either also works as the password
  for `admin`;
- an HttpOnly `stormd_session` cookie from a UI login (sessions are in
  memory, 24 h, gone on restart).

Credentials are compared in constant time. Open paths: `/api/v1/health` and
`/healthz` (the probes), and what the login screen needs: `/`, `/ui/*` except
`/ui/proxy/*` (the static SPA, no data) and `/api/v1/auth/*`. `/metrics` is
not open (#32). The TLS certificate and key are re-read when either changes;
a pair that fails to load keeps the previous one and logs a warning. The
client CA is read at start. stormsh passes the token with `-t`/`--token`,
`--token-file` or `STORMD_TOKEN`, and speaks https with `--ca-file` (below).

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
stormsh [-H HOST] [-p PORT] [-t TOKEN | --token-file FILE]
        [--ca-file CA.pem [--cert CERT.pem --key KEY.pem]]
                                          # defaults 127.0.0.1, 9080, $STORMD_TOKEN, http
```

`--ca-file` switches to https and trusts only that CA (the node CA for a
stormd with `[api] tls_cert_file`); `--cert`/`--key` present a client
certificate for `[api] client_ca_file`. With TLS, `-H` must be a name or
address in stormd's certificate.

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

**On a stormcos node** the default `cloudid_url` does not reach cloudid: the
node puts `169.254.169.254/32` on `lo` and stormimds answers it. stormimds
knows only registered guests, so it answers a node's stormd 404, and it serves
no `public-keys/` yet (stormimds#5). Which service should answer a node's own
processes is undecided (stormimds#9); until then the refresh gets no keys
there (#30). No stormcos golden sets `[ssh] owner` today, so no node runs the
refresh.

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

**Exit status:** `false` exits 1 and an unknown name exits 127 (without
starting init, #11). Every other
applet exits **0, even when it fails**: `stat /missing` prints the error and
exits 0, and so does `grep` with no match. So an applet cannot express "wait
until this file exists" as a one-shot or an exec probe (#31).

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

## Goldens

A process can be given stormblock goldens, read-only (#36; minismbd#11 option
A: the service serves plain paths and never talks to stormblock):

```toml
[[process.golden]]
name = "nic-drivers"                        # path defaults to <[goldens] dir>/<name>
golden = "golden-nic-drivers-56ea4782ef2a"  # or volume_id = "<uuid>"
content = "filesystem"                      # or "image"
```

| Key | Default | |
|---|---|---|
| `name` | required | unique in the process, no `/` |
| `golden` \| `volume_id` | one required | the golden's name in stormblock, or its volume id |
| `content` | required | `filesystem` — mounted read-only (`MS_RDONLY`, `nodev`, `nosuid`) at `path`; `image` — a block device node at `path` |
| `path` | `<dir>/<name>` | absolute |
| `fstype` | `ext4` | filesystem goldens |
| `owner`, `mode` | root, `0o444` | image goldens: `uid:gid` and mode of the node, so an unprivileged service can read it |
| `size_bytes` | — | image goldens: the image's own length (the volume is larger); reported in `/api/v1/goldens`, the service limits reads to it |

`[goldens]`: `engine_url` (default `http://${NODE_IP}:9090`, this node's engine:
a ublk attach is local), `token_file` (default
`/run/stormblock/engine/api_token`, re-read on every call; absent → no
`Authorization`), `dir` (default `/goldens`).

**How:** before the process's first start (after `depends_on` and
`wait_for_files`), stormd resolves a `golden` name through `GET
/api/v1/volumes?kind=golden`, asks `POST /api/v1/volumes/{id}/attach` with
`{"mode":"ro","transport":"ublk","holder":"stormd/<container>/<process>/<name>"}`,
makes the device node from `/sys/block/<dev>/dev` (a container's `/dev` is a
tmpfs without it), and mounts or places it. A failure is retried every 2 s,
one warning per distinct error; the process does not start until all its
goldens are presented, and shutdown ends the wait. At shutdown, after the
processes stop, each is unmounted (or its node removed) and detached.

**Swap** (a new release): `PUT /api/v1/processes/{p}/goldens/{name}` with the
new `golden` or `volume_id` stops the process, releases the old golden,
presents the new one and starts the process again. If the new one cannot be
presented, the old one is put back. The swap is in memory: a stormd restart
presents what the config says.

**What it needs from the container** (stormcos wires these per service): the
engine's token (a `mount sbrun /run/stormblock ro` stanza, as sbregistry
has), reach to the engine on `:9090`, and stormd running as root with
`CAP_SYS_ADMIN`/`CAP_MKNOD`: stormdbase containers have both today, since
stormpump drops no capabilities, and boot.d services keep them under the decided default (stormpump#47). Only the local ublk transport
is used; an NVMe-TCP answer is an error. Processes owned by the image updater
are not given goldens.

## Development notes

- `web/dist` is committed; rebuild it when `web/` or stormview changes.
- New dashboard content: add a summary source in `components.rs`; no frontend
  change.
- Design notes and history: [docs/](docs/). Changes: [CHANGELOG.md](CHANGELOG.md).
