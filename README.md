# stormd

A container init for scratch images: one static binary that is PID 1,
supervises one or more processes, keeps their logs, and answers questions
about them over a REST API, a web console, SSH and a TUI client.

In stormcos, stormd is PID 1 of every supervised component container —
fastetcd, the rustkube control plane, the kubelet, stormdrive, stormstorage,
stormconsole, cadvisor, stormlb and the rest (see [How it ships](#how-it-ships)).

This README is written from the code at v0.8.0 (refreshed 2026-10-09). Where something is parsed but
does nothing, or does something other than it says, it says so and names the issue.

A 12-slide overview is in [docs/presentation.md](docs/presentation.md) (Marp:
`npx @marp-team/marp-cli docs/presentation.md`).

## What it does today

- **Supervises processes** — start order with `depends_on` and ready probes,
  restart policies for crashes (`on_failure`) and clean exits (`on_exit`), an
  escalating restart delay capped at 30 s, a restart budget per window, and
  exit codes a process can declare not worth retrying (`no_restart_exit_codes`).
  A one-shot (`on_exit = "stop"`) satisfies its dependents once it has exited 0.
- **Startup, liveness and readiness probes, the Kubernetes way** —
  `http_get` / `tcp_socket` / `exec`, with Kubernetes' fields and defaults.
  The startup probe gates the others, liveness restarts, readiness only
  marks not ready (#48).
- **Fills in node values** — `${NODE_IP}` and `${NODE_NAME}` in a process's
  `args` and `env` values are expanded each time it is spawned.
- **Node-overridable defaults** — `env_default` entries apply only when stormd
  did not inherit the key, so a node's env.d can override them.
- **API health** — a process declares its real APIs; stormd times them
  against a budget (`healthy`/`slow`/`stalled`/`down`), logs each change
  loudly, keeps it, and restarts on a long stall only when told to (#49).
- **Goldens** — a process can name stormblock goldens; stormd attaches each
  read-only and presents it (a filesystem golden mounted read-only, an image
  golden as a readable device node) before the process starts, and swaps one
  for another at runtime (#36).
- **Logs** — stdout/stderr per process to a rotated file on the log volume,
  each run's file kept (and pruned) when it exits, every line on the fleet's
  multicast syslog group (repeats collapsed, rate-limited per process) (the [stormcast](https://github.com/glennswest/stormcast)
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
- **Busybox-style multi-call binary** — 65 commands through `argv[0]` symlinks,
  so a scratch container has `ls`, `cat`, `curl`, `ping`, … .
- **Cron** — 6-field (seconds-first) schedules.
- **Log backup** — tar(.gz) the log directory and POST it somewhere when the
  container fails, or on demand.
- **OCI image updater** — processes with an `image` are pulled, unpacked into a
  rootfs directory, and swapped when the registry digest changes.
- **PID 1 duties** — reaps zombies, shuts down on SIGTERM/SIGINT (bounded:
  dependents first, each SIGTERM then SIGKILL, and a deadline it exits by
  whatever stalls — see [Running](#running)), writes a few network sysctls, and has
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
  src/probes.rs       startup/liveness/readiness probes (Kubernetes-shaped)
  src/k8sevents.rs    Kubernetes-shaped events, /api/v1/events
  src/apihealth.rs    [[process.api]] health probes
  src/goldens.rs      [[process.golden]]: attach, present, swap, release
  src/tls.rs          API TLS, client certificates, file rotation
  src/cron.rs events.rs backup.rs updater.rs cloudid.rs stats.rs debug.rs
crates/stormlog/  logging: rotated files, multicast emit, VT100, streams
crates/stormsh/   TUI client (ratatui)
test/             stormd-test: the test container (short/medium/long suites)
web/              Svelte 5 SPA source; web/dist is the built output (committed)
config/           example.toml — every key, parsed by a unit test
docs/             plugin UI guide, design notes
vendor/           vendored russh-sftp
```

Versions: stormd 0.8.0, stormsh 0.5.0, stormlog 0.4.0 (each crate's
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

**The test image** follows stormcentral's
[test standard](https://github.com/glennswest/stormcentral/blob/main/docs/test-standard.md)
(#24): one image for every suite, started as `/test <suite>` (`short`,
`medium` or `long`; `STORM_SUITE` when no suite is given), built from
`test/Containerfile` with the repo root as context, run by stormcentral as a Job in the run's own namespace
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
| `medium` | < 30 min | a failed one-shot holds its dependents, and SIGTERM still stops stormd; `no_restart_exit_codes` hold and fail; `on_failure = "fail"`; `max_restarts`; `on_exit = "restart"`; a liveness probe that keeps failing restarts; API stop/start/restart and shutdown with an exit code; bearer-token auth; `/metrics` (with the worker's own RSS, CPU and fds); the component feed; cron; a config that does not parse exits 1; run as `ps` (not an applet), exits 127 and spawns nothing; `wait_for_files` holds the start until the file exists; a taken API port exits 1 with nothing started; `restart?wait=healthy` answers 200 only after the new run's probe and 504 naming what it waits on |
| `long` | the night window | waves of processes sized from the pod's own CPU, memory and pid limits (mostly long-running, some crash-once, one-shots with dependents), started, settled and stopped with SIGTERM; one resident stormd has its processes restarted through the API every wave. Per wave: settle time, stop time, leftover processes, the resident's RSS and fds. A wave twice as slow as the first of its size, a leftover, or growing residue fails |

Build it on the build box (stormd needs `stormpull` over `ssh://`, so the
binaries are built by cargo there, not inside a container build):

```bash
test/build.sh                         # static musl binaries → test/out/{stormd,stormd-test}
```

stormcentral's runner runs that, then makes the image from
`test/Containerfile` itself (no container runtime): `FROM scratch`, the two
binaries copied from `test/out/` as `/stormd` and `/test`.

Run it by hand against a cargo build — it finds `stormd` next to itself, or
`STORMD_BIN`; scratch goes to a temp directory when there is no `/results`:

```bash
target/debug/stormd-test short
STORM_TIMEOUT=600 target/debug/stormd-test long
```

## How it ships

stormd is an **input golden**: stormcentral's component registry lists it as
`kind = "input"` (`golden-stormd-<id>`), and `/stormd` inside every
*stormdbase* golden — the base each supervised component golden is built on —
is taken from it.

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

So **a commit here reaches a node once it is in a stormd input golden** and
a stormcos release carries the goldens built on it. The input golden is
recorded after each stormd issue whose work passes sc-build (`stormcentral
component build stormd`, #43). Nothing rebuilds it otherwise: until 2026-10-07
every golden still carried stormd@8edb89c (09-29).

### stormd's API port on a node

Every stormd on a node shares the host network, so each has its own port:

| Container | stormd API |
|---|---|
| fastetcd | 9081 |
| rustkube-apiserver / controller-manager / scheduler | 9082 / 9083 / 9084 |
| rustkube-node (kubelet, kube-proxy) | 9085 |
| `kind = "service"` goldens (stormdrive, stormstorage, stormconsole, cadvisor, stormlb, …) | the service's port + 100 (stormdrive 9192, stormstorage 9193, stormconsole 9194) |

A service golden's config also sets `no_restart_exit_codes = [78]` and an HTTP
liveness probe on the service's health path. A config that still writes that
probe as the old `[process.liveness]` gets no liveness at all since #48 (the
key is retired: parsed, warned, never acted on); stormcos moves its goldens to
`[process.liveness_probe]` / `startup_probe` (stormcos#186).

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

Invoked through a symlink whose name is one of the 65 applets, stormd runs
that command and exits instead (see [Busybox commands](#busybox-commands)).
stormd starts as init only when the basename of `argv[0]` is `stormd`, or a
renamed copy (`stormd-*`, `stormd.*`). Under any other name — `/bin/ps`
linked to stormd, say — it prints `stormd: ps: not a stormd applet (see
stormd --list-commands)` and exits 127 before reading a config or starting
anything (#11).

Startup, in order: install applet symlinks into `/bin`, `/usr/bin`, `/sbin`,
`/usr/sbin` (skipping names that exist; errors ignored) → load and validate the
config (exit 1 on error) → resolve the cloud ID → start logging → load the
API's TLS pair and bind the API (exit 1 on either, **before anything is
started**, #23) → start cron and the updater → start processes → start the
SSH server (in the background; a failed bind is logged and stormd carries
on) → serve the API → reap zombies and set sysctls (Linux). A port already
taken (another stormd, a stale process) therefore leaves nothing running.

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
after shutdown began is recorded as a stop, not a crash. The signal handler
starts shutdown first thing. Something that signals the whole process group
(`timeout`, a terminal's Ctrl-C) can kill a child before that handler runs.
So a child that died of SIGTERM, SIGINT or SIGHUP is given up to 300 ms for
shutdown to begin, and is then a stop, not a crash with a restart scheduled
(#26).

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
An unknown key, including a removed one (`[log]`, `[general] pid_file`,
`[debug] dynamic_log_level`, `[updater] registry`), is logged as one WARN
each (`unknown config key — ignored`) and does not stop stormd (#7). `config/example.toml` shows every key and is
parsed by a unit test.

Validation at load (any failure: exit 1, nothing started): at least one
`[[process]]` or `[[cron]]`; process names unique; each process has `command`
or `image`; every `depends_on` names a process; `wait_for_files` entries are
absolute; each startup/liveness/readiness probe has exactly one action,
`period_seconds`, `timeout_seconds` and both thresholds ≥ 1,
`success_threshold = 1` unless it is a readiness probe, a non-empty
`exec.command` and an `http_get.scheme` of HTTP or HTTPS; a
`[[process.api]]` has a unique non-empty name, an `http://`/`https://` url,
`interval_secs`/`timeout_secs` ≥ 1, `client_cert_file` and
`client_key_file` together, and `restart_after_stalled_secs` ≥ 1 if set; a
`[[process.golden]]` has a unique name without `/`, exactly one of
`golden`/`volume_id`, an absolute `path` if any, and `owner` as `uid:gid`;
`[events]` enabled with `transport = "webhook"` needs `webhook_url`;
`[backup] enabled` needs `destination_url`; `[api] tls_cert_file` and
`tls_key_file` go together, and `client_ca_file` needs them.

### `[general]`

| Key | Default | |
|---|---|---|
| `name` | `"stormd"` | container name: UI, events, metrics `container` label |
| `log_dir` | `"/var/log/stormd"` | per-process log files and `.cloudid`; created at start (exit 1 if it cannot be). **Overrides `[stormlog.file] log_dir`** (a different value there is logged as overridden). Put it on a volume: in a stormcos golden it is the component's `-logs` volume; a stormd run as a pod wants a PVC (on stormcos, the built-in stormblock PVC driver) — a container-local path is RAM on some hosts |
| `cloud_id` | — | see [Cloud ID](#cloud-id) |
| `theme` | — | default web UI theme id (below); a viewer's own pick wins |

Theme ids (from stormview): `storm`, `one`, `gruvbox`, `catppuccin`, `rose`,
`midnight`, `nord`, `solar`, `phosphor` (dark); `light`, `frost`, `paper`
(light).

### `[api]`

| Key | Default | |
|---|---|---|
| `bind` | `"0.0.0.0:9080"` | REST API, WS, metrics and UI |
| `tls_cert_file`, `tls_key_file` | — | PEM certificate chain and key: serve the API over TLS (HTTP/1.1). Both or neither; re-read when either file changes |
| `client_ca_file` | — | a PEM CA bundle or a **list** of them, e.g. the node CA and forge's CA (#59). Needs TLS. A client certificate that verifies against any of them authenticates the request; one that verifies against none fails the handshake. Each file is re-read when it changes; a missing or unreadable one is skipped with one WARN; with none readable no client certificate is accepted (clients without one still connect, anonymously) |
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
`client_ca_file` (the node CA and, for stormcentral, forge's CA, #59), and a
`token_file` (#32). A missing, unreadable or mismatched pair stops stormd at
start (exit 1), before any process is spawned. A missing client CA file does
not: it is skipped, and taken up when it appears.

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
| `[process.startup_probe]`, `[process.liveness_probe]`, `[process.readiness_probe]` | — | Kubernetes-style probes, below |
| `[process.liveness]` | — | **retired** (#48): parsed, logged as retired, never acted on |
| `restart_policy` | — | `Always` \| `OnFailure` \| `Never` (Kubernetes): replaces `on_exit`/`on_failure`/`restart_delay_secs`/`max_restarts` — below |
| `[process.ui]` | — | plugin tab, below |
| `capture_stdout`, `capture_stderr` | `true` | `false`: that stream goes to /dev/null instead of the log (#7) |
| `[[process.golden]]` | — | goldens presented to the process, below and [Goldens](#goldens) |
| `[[process.api]]` | — | the process's APIs, probed for health — [API health](#api-health) |

**`ready_probe`** — `{ type = "http", url = "...", interval_secs = N }`,
`{ type = "tcp", port = N, interval_secs = N }` or
`{ type = "exec", command = "bin arg ...", interval_secs = N }`
(`interval_secs` is required). Polled in the background after each spawn,
5 s timeout per attempt; HTTP passes on 2xx/3xx (certificates not verified),
TCP connects to `127.0.0.1:port`, exec passes on exit 0. It gates dependents
only. Applets exit non-zero when they fail (see [Busybox commands](#busybox-commands)),
so `{ type = "exec", command = "/bin/test -e /etc/stormcert/x.crt" }` waits for
a file (#31).

**`[process.startup_probe]` / `[process.liveness_probe]` /
`[process.readiness_probe]`** (#48) take the fields of a Kubernetes container
probe, with Kubernetes' meaning and defaults (camelCase spellings such as
`periodSeconds` and `httpGet` are accepted too):

| Key | Default | |
|---|---|---|
| `http_get` | — | `{ path = "/", port, host = "127.0.0.1", scheme = "HTTP"\|"HTTPS", http_headers = [{name, value}] }`: passes on 200–399; certificates not verified, redirects not followed |
| `tcp_socket` | — | `{ port, host = "127.0.0.1" }`: passes if it connects |
| `exec` | — | `{ command = ["/bin/test", "-e", "/x"] }`: passes on exit 0 |
| `grpc` | — | `{ port, service }`: `grpc.health.v1.Health/Check` over plaintext HTTP/2 on 127.0.0.1, passes only on `SERVING` (a TCP connect passes while a server is up but not serving) |
| `initial_delay_seconds` | `0` | from the spawn |
| `period_seconds` | `10` | |
| `timeout_seconds` | `1` | |
| `success_threshold` | `1` | consecutive passes (must be 1 for startup and liveness) |
| `failure_threshold` | `3` | consecutive failures |

Exactly one of `http_get` / `tcp_socket` / `exec` / `grpc`.

The old `[process.liveness]` is **retired**. It had no startup grace and
killed slow starts: the apiserver six times during a normal 40 s start on the
Dell, and fastetcd while opening its data. It still parses, logs a warning at
each spawn, and is never acted on.

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
| `timeout_secs` | `300` | after this the job is killed (SIGKILL) and the run recorded as failed |
| `capture_output` | `true` | once the job ends, its stdout/stderr are logged as process `cron.<name>` (stderr as warnings) |

Each job keeps its next fire time and runs when it comes (`GET /api/v1/cron`
shows it as `next_run`). Each run is on its own task, so a long job does not
hold up the others (#10). A job whose previous run is still going skips a
fire time, with a WARN, rather than piling up runs.

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
| `authorized_keys` | — | an OpenSSH `authorized_keys` file (lines `type base64 [comment]`, options allowed), read at every login, accepted alongside CloudID's keys (#7) |

### `[debug]`

| Key | Default | |
|---|---|---|
| `enabled` | `false` | adds `GET /api/v1/debug/info` and `/api/v1/debug/config` |
| `allow_signal` | `false` | adds `POST /api/v1/debug/processes/{name}/signal` |
| `allow_stdin` | `false` | adds `POST /api/v1/debug/processes/{name}/stdin` |

### `[stormlog.file]`, `[stormlog.mcast]`, `[stormlog.terminal]`

| Key | Default | |
|---|---|---|
| `file.max_size_bytes` | `104857600` (100 MiB) | rotate `<process>.log` at this size |
| `file.max_files` | `10` | rotated generations kept per process |
| `file.max_runs` | `10` | finished runs kept per process |
| `file.log_dir` | — | **ignored**: always `[general] log_dir`; a different value is logged as a WARN at start |
| `mcast.group` | `239.255.42.1:5514` | `host:port`, or `"off"` / `""` to send nothing (#27). An address that does not parse sends nothing, with a WARN |
| `mcast.host` | this machine's hostname | syslog HOSTNAME field (the node, not the container) |
| `terminal.rows` / `cols` | `24` / `80` | VT100 screen per process |
| `terminal.scrollback` | `1000` | lines |

## Process supervision

Processes without `image` are started at boot **in config order**; each first
waits for its `depends_on` (woken the moment a dependency's state or
readiness changes, with a 1 s poll as a backstop — #25), then its `wait_for_files`, then `startup_delay_secs`,
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

**`restart_policy`** (#48), as in Kubernetes. When it is set, it decides
alone, and `on_exit`, `on_failure`, `restart_delay_secs` and `max_restarts`
are not used:
- `Always` restarts every exit, `OnFailure` every failed one (an exit code
  other than 0, or a signal, which includes a liveness kill), and `Never`
  none. A process left down is `stopped` after exit 0 and `failed`
  otherwise. It never fails the container.
- Each restart waits out an exponential back-off: 10 s, 20 s, 40 s … capped
  at 5 min. The process is `CrashLoopBackOff` meanwhile, and the restart
  count is kept. A run that lasted 10 minutes resets the back-off.
- There is no restart limit, as upstream. An exit code in
  `no_restart_exit_codes` still holds the process.

**A failed run's last words** (#29). When a process exits with an error,
stormd writes the last 20 lines it printed in that run (stdout and stderr
together) to its own stderr, prefixed `name| `, just before `process exited
with error`. In a container stormd is PID 1, and its output is what the node
keeps (stormpump's console, stormpump.log, `assets.json`). Without this, the
cause of a crash sat only in a log file on the container's own volume.

**Probes** (#48), per run, ending with the run (the #45 lesson):
1. **Startup**, if any, runs first; liveness and readiness wait for it. It
   gets `failure_threshold × period_seconds` to succeed once. If it runs out,
   the run is killed (SIGTERM, then SIGKILL after `stop_timeout_secs`) and
   its exit goes through the table above.
2. **Liveness**: `failure_threshold` consecutive failures kill the run the
   same way (`Killing: Container … failed liveness probe, will be
   restarted`).
3. **Readiness**: `success_threshold` passes mark the process ready,
   `failure_threshold` failures mark it not ready. It is never restarted.

**Events** (#48). Every probe failure, kill, back-off and start is recorded
with Kubernetes' reasons and wording, and de-duplicated as the kubelet does
it: the same reason and message again raises `count` and `lastTimestamp`.
- `Created` / `Started`: `Started container <name>`.
- `Unhealthy`: `Liveness probe failed: dial tcp …`.
- `Killing`: `Container <name> failed liveness probe, will be restarted`,
  or `Stopping container <name>`.
- `BackOff`: `Back-off restarting failed container <name>`.

The last 1000 are kept and served at `GET /api/v1/events?since=<seq>`.
rustkube-node puts them on the process's mirror pod (`kubectl get events`).

A process with any of these starts not ready. With a startup probe and no
readiness probe, it is ready once the startup probe succeeds. Readiness is
what `depends_on` waits for. Each failure is logged as `Unhealthy: <kind>
probe failed: <message>`, in Kubernetes' wording.

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
   explains a crash is in the file. The file is opened for every line. If
   the directory has gone (removed, a mount that came late) it is created
   again and the open retried. If the file still cannot be opened or
   written (disk full, permissions, a mount gone), stormd says so once,
   tries again at most every second, and repeats the ERROR at most once a
   minute with a count. The lines in between still reach the group and the
   streams; only the file misses them. When the file works again, stormd
   logs that, and the file gets one line saying how many lines it is
   missing (#1).
2. **The fleet's multicast group** — RFC 5424 syslog over UDP, framed by
   [stormcast](https://github.com/glennswest/stormcast) (shared with
   stormpump). Send only; collecting and searching a fleet's logs is
   mcastsyslog's job. Lines pass stormcast's limiter, one per process, as
   in stormpump on the host (#12). Repeats collapse into `last message
   repeated N time(s)`. Past a burst of 2000, more than 200 lines a second
   are dropped, and a count follows (`N message(s) dropped — over 200
   lines/s`). Both notices go out as Notice lines from the same process. A
   run of repeats still held when the process's output ends is sent then.
   stormd's own `*** PROCESS CRASHED ***` line is never rate-dropped. Only
   the group is limited: the file and the live streams keep every line. stormcast is pinned in
   `Cargo.lock` at 0.1.1 (`3cec734`). It cuts a line over 8 KiB at a character
   boundary; the 0.1.0 it replaced panicked PID 1 when byte 8192 fell inside
   a multibyte character (#28, stormcast#4). Since stormcast#5, HOSTNAME and
   APP-NAME are each one RFC 5424 token: a process name with a space or a
   non-ASCII character is sent with `_` in its place.
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
children's — except when a run fails: then the last 20 lines that run printed
are echoed there, prefixed `name| `, just before `process exited with error`
(#29, see [A failed run's last words](#process-supervision)). So a host
supervisor that keeps only stormd's output (stormpump, on a node console)
sees why it failed, not only that it did.

## Events

Kinds: `container_starting`, `container_stopping`, `container_failing`,
`process_started`, `process_stopped`, `process_crashed`, `process_restarting`,
`liveness_check_failed`, `cron_executed`, `cron_failed`, `backup_started`,
`backup_completed`, `backup_failed`, `update_check_started`,
`update_available`, `update_pulling`, `update_pivoting`, `update_completed`,
`update_failed`, `process_ready`. `process_ready` is emitted when a process
becomes ready (a passing readiness or `ready_probe`, or a startup probe with
neither) (#7); `liveness_check_failed` when a `liveness_probe` reaches its
`failure_threshold` and the run is killed. These are stormd's own events;
the Kubernetes-shaped ones (`Unhealthy`, `Killing`, …) are separate, at
`GET /api/v1/events` (see [Process supervision](#process-supervision)).

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
| POST | `/api/v1/processes/{name}/start` \| `stop` \| `restart` | `restart?wait=healthy&timeout=N`: answer once the new run is healthy (200), it ended (502), or N s passed (504, process left running) — below |
| GET | `/api/v1/events?since=<seq>` | Kubernetes-shaped events (`type`, `reason`, `message`, `process`, `count`, `firstTimestamp`, `lastTimestamp`, `seq`), changed after `seq` — below (behind auth) |
| GET | `/api/v1/health/apis` | every declared API's health: state, since, last latency, p50/p99 seen, budgets, last error (behind auth) |
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
| `stormd_process_state{state}` | gauge | 1 for the current state of `running`, `stopped`, `failed`, `starting`, `restarting`, `crashloopbackoff` |
| `stormd_process_restarts_total` | counter | |
| `stormd_process_crashes_total` | counter | non-zero exits |
| `stormd_process_resident_memory_bytes` | gauge | the running process's RSS (`/proc/<pid>/status` VmRSS) |
| `stormd_process_virtual_memory_bytes` | gauge | its VmSize |
| `stormd_process_cpu_seconds_total` | counter | its user + system CPU (`/proc/<pid>/stat`), per run: it starts over when the process restarts |
| `stormd_process_open_fds` | gauge | entries in `/proc/<pid>/fd` |
| `stormd_process_liveness_failures_total` | counter | every liveness probe failure since stormd started (#10) |
| `stormd_process_liveness_consecutive_failures` | gauge | failures in a row now; 0 after a pass (what `failure_threshold` counts) |
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
there (#30). A 404 is logged once as a WARN that names that likely cause.
No stormcos golden sets `[ssh] owner` today, so no node runs the refresh.

A session is an interactive shell (a PTY is expected); `ssh host command`
(exec requests) is not supported. The `sftp` subsystem is, so `sftp` and
OpenSSH's default (SFTP-based) `scp` work; legacy `scp -O` does not.

The shell, in addition to every applet below:

```
ps / top            processes with state and liveness (*)
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

(*) The liveness column, `liveness` and the `status` summary still read the
retired `[process.liveness]`, so a process with only a `liveness_probe`
shows none there; `/metrics` has its counts (#56).

Tab completion (commands, process names, paths), history, `|` pipes, and
`>` / `>>` redirection.

## Busybox commands

`argv[0]` dispatch, 65 commands:

| | |
|---|---|
| File | `ls dir cat head tail cp mv rm mkdir touch chmod chown find ln stat pwd wc du readlink file sha256sum md5sum tee` |
| Network | `ifconfig ip ping curl wget netstat ss nslookup dig hostname route` |
| System | `mount df free uname date id kill printenv export unset sleep echo env whoami which type lsof true false clear` |
| Text | `sort uniq cut tr sed rev base64 xxd grep` |
| Tests | `test [` — `-e -f -d -s -r -w -x PATH`, `-n`/`-z STR`, `A = B`, `A != B`, leading `!` |

`stormd --install DIR` links them all to the running binary; stormd also does
this at every start for `/bin`, `/usr/bin`, `/sbin` and `/usr/sbin`. Piped
stdin works (`ls /app | grep server`).

**Exit status** (#31), as in coreutils and busybox: 0 on success, 1 on
failure, 2 on bad usage (a missing argument). Some examples:
- `stat /missing`, `cat /missing` and `ls /missing` exit 1. So does a `cat`
  of several files where any one is missing; the rest are still printed.
- `grep` with no match exits 1, `-c` included. `test` exits 1 when false.
- `rm -f` of something absent exits 0. `ping` exits 1 when nothing answered.
- `false` exits 1. A name that is not an applet exits 127 without starting
  init (#11).

Error text is printed on stdout with the rest, as before. So a one-shot
(`command = "/bin/test"`, `args = ["-e", "/path"]`, `on_failure =
"restart"`) or an exec probe can wait for a file.

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

Each successful pull and pivot records how it started the process (image,
digest, command, env, working directory) in `<rootfs_dir>/<name>.image.json`.
When stormd starts and finds a rootfs with that record, it starts the process
from it at once (#8). The record's digest counts as the current one, so if
the registry has something newer the first poll pulls it. A rootfs without a
record (left by an older stormd) is pulled again. With `[updater] enabled =
false` an `image` process never runs, and stormd logs one ERROR per such
process at start saying so.

## Restart that waits for health

`POST /api/v1/processes/{name}/restart?wait=healthy&timeout=N` (default 60 s,
at most 3600) answers once the *new* run is healthy (#44). This is for a
caller like stormcert, which rotates a certificate and must say when the
service is back.

A run is **healthy** once it is running and has passed every check it has,
counted from this spawn:
- its `ready_probe`;
- its liveness probe;
- each `[[process.api]]`, `healthy` on a probe made after the spawn.

With none of those, it is healthy after 3 s of running.

| Answer | When |
|---|---|
| 200 `{"status":"healthy","run":N,"waited":ms}` | healthy |
| 504 `{"status":"timeout","run":N,"waiting_on":[…]}` | `timeout` passed first; the process is **left running** for the caller to decide |
| 502 `{"status":"exited","run":N,"exit_code":…,"state":…}` | the run ended while it was waited for |

Each answer also carries `"process"`.

`GET /api/v1/processes/{name}` reports `run`, `ready` / `ready_at`,
`liveness_passed_at` and `liveness_passed_run`, and `healthy`. A caller that
lost the connection can tell from these whether *this* run passed. A
restarted process's `ready_probe` is now watched again for each run: before
#46 a restart was never `ready` again.

## API health

Liveness asks a cheap `/healthz`, and that answers while the real work is
stuck. On 2026-10-08 a storage engine held its volume mutex through a
six-minute build, and every API call behind it stalled unnoticed
(stormblock#358, stormcos#458). So a process can declare the APIs it serves,
each with a cheap *real* read:

```toml
[[process.api]]
name = "volumes"
url = "http://127.0.0.1:9090/api/v1/volumes?limit=1"
p50_ms = 50
p99_ms = 500
token_file = "/run/stormblock/engine/api_token"
# restart_after_stalled_secs = 300
```

| Key | Default | |
|---|---|---|
| `name` | required | unique in the process |
| `url` | required | `http://` or `https://`, GET (certificates not verified, redirects not followed) |
| `interval_secs` | `15` | between probes |
| `timeout_secs` | `5` | no complete answer (headers *and* body) within this → `stalled` |
| `p50_ms`, `p99_ms` | — | budgets: an answer over `p99_ms`, or a p50 of the last 20 answers over `p50_ms`, is `slow` |
| `initial_delay_secs` | `10` | after each start, before the first probe |
| `token_file` | — | bearer token, re-read every probe |
| `client_cert_file` + `client_key_file` | — | a client certificate (PEM) |
| `restart_after_stalled_secs` | — (never) | stalled this long → restart |

**States:**
- `healthy`: answered, within budget.
- `slow`: answered, over budget.
- `stalled`: no answer within the timeout.
- `down`: refused, a connection error, or an HTTP status of 400 or more.
  A 401 means the probe's credentials are wrong, which is reported rather
  than passed as healthy.
- `unknown`: before the first probe.

**Logging:** each change is logged once. `healthy` is INFO, `slow` WARN, and
`stalled`/`down` ERROR. The line names the process, the API, the URL, the
latency or error, the state before and how long it lasted. Each change is
appended to `/system-data/history/api/<process>.jsonl` when
`/system-data/history` exists (stormcos#456). `GET /api/v1/health/apis`
serves the current state of every API.

**Published for the node** (#52). With `[api_health] state_file = "…"`
(on stormcos `/run/stormpump/health.d/<container>.json`, a directory PID 1
makes and stormcos binds in), stormd writes the current state to that file
after every probe, changed or not. The file holds the
`/api/v1/health/apis` body plus a top-level `updated` and each API's
`interval_secs`. It is written to `<file>.tmp` and renamed, written empty at
start, and left in place on stop. PID 1 merges it into the node's health
without TLS (stormpump#127). It ages the file by its modification time: one
older than 3 × the largest `interval_secs` counts as stalled, so a hung
stormd cannot look healthy.

**Acting** is opt-in, per API. With `restart_after_stalled_secs = N`, an API
stalled for N seconds is logged as such, and the process is sent SIGTERM
(SIGKILL after its `stop_timeout_secs`). Its exit then goes through the
restart policy: it counts as a crash, with cool-off and `max_restarts`.
Without the key nothing is ever done. The probes run one task per API per
run and end with the run. The state is kept per process and API across
restarts.

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
