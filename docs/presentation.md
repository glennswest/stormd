---
marp: true
theme: default
paginate: true
title: stormd — the init inside every stormcos component
description: Purpose and functionality of stormd v0.7.0, from the code
---

<!-- Render: npx @marp-team/marp-cli docs/presentation.md          (HTML)
             npx @marp-team/marp-cli --pdf docs/presentation.md    (PDF)
     Written 2026-09-24 against stormd v0.7.0. Every claim is checkable in
     the source; README.md has the full reference. -->

# stormd

### The init inside every stormcos component container

v0.7.0 · github.com/glennswest/stormd

---

## What it is, and the problem it solves

A component shipped as **a static binary alone** has no one watching it. Nothing
restarts it except the whole container. Nothing checks whether it is *healthy*
rather than merely running. Its output goes wherever the sandbox points. And
nobody can ask it anything from outside.

**stormd is PID 1 of the container.** It starts the component, restarts it
according to the policy in its config, probes it, keeps its logs, puts every
line on the fleet's multicast group, and answers over REST, a web console,
SSH and a TUI.

So each component golden is **stormd + one binary + a config**, and every
component gets all of this, the same way.

---

## Where it sits in stormcos

From stormcentral's relationships graph (`config/stormcentral.toml`):

```
  stormcast ─┐                 ┌─ rustkube, stormcert, stormlb      (control plane)
  (log wire) ├──▶  stormd  ◀───┼─ stormimds, stormipmi              (node)
  stormview ─┘   (tooling)     ├─ stormdrive, stormstorage,
  (UI contract)                │  stormblock-registry               (storage)
                               ├─ stormcoredns                      (network)
                               ├─ stormconsole                      (ui)
                               └─ stormcos  — builds every golden around it
```

- **Also pinned at build time:** stormpull (image pulling, from the stormbase repo).
- **Also runs under stormd, with no edge in the graph (stormcentral#24):** fastetcd,
  rustkube-node and cadvisor. Their goldens are stormdbase goldens too.
- **Not under stormd:** stormpump (the host PID 1), stormblock and the
  registry binary, which run bare.

---

## How it works

```
                      /etc/stormd/config.toml
                                │
   ┌────────────────────────────▼─────────────────────────────┐
   │ stormd (PID 1)                                           │
   │                                                          │
   │  supervisor ── spawn ─▶ [process] … [process]            │
   │   │ policy, probes,        │ stdout/stderr pipes          │
   │   │ ${NODE_IP}             ▼                              │
   │   │               stormlog ─┬─▶ {log_dir}/<proc>.log      │
   │   │                         ├─▶ 239.255.42.1:5514 (UDP)   │
   │   ▼                         └─▶ VT100 + live streams      │
   │  events ─▶ log (always) / webhook                        │
   │  cron · updater · backup                                 │
   │                                                          │
   │  axum :9080 ── REST · WS · /metrics · /ui (Svelte SPA)   │
   │  russh :22  ── shell · SFTP              (when enabled)  │
   └──────────────────────────────────────────────────────────┘
        ▲ /api/v1/components ── web dashboard + stormsh
```

---

## Supervision — what it does today

- **Start order.** Processes start in config order. `depends_on` waits
  until a dependency is running *and* its `ready_probe` (http, tcp or exec)
  has passed, or until a one-shot dependency (`on_exit = "stop"`) is done.
- **Restart policy.** `on_failure` is `restart`, `fail` or `ignore`.
  `on_exit` is `restart` or `stop`. `max_restarts` counts restarts within
  `restart_window_secs`.
- **Back-off.** The delay is `restart_delay_secs × 2^(n-1)`, capped at
  30 s, so a process that can never start is retried every 30 s, not every
  second.
- **Non-retryable exits.** `no_restart_exit_codes = [78]` marks the process
  failed and does not restart it. By default it leaves the container
  running (`on_no_restart = "hold"`).
- **Liveness.** An HTTP or TCP probe. After `failure_threshold` misses,
  SIGUSR1, then 5 s, then SIGKILL.
- **`${NODE_IP}` / `${NODE_NAME}`** in args and env are filled in at every
  spawn, so a control plane advertises an address other nodes can reach.

---

## Logs and events — what it does today

- Every line goes to **three places**:
  - a rotated file per process;
  - the fleet's **multicast syslog group**, in stormcast's framing, shared
    with stormpump;
  - live streams (VT100 screens, WebSockets).
- **Per-run files.** When a run ends, its file is renamed
  `<proc>.<run>.<failed|exited>.log`. Old runs are pruned (`max_runs`), so
  a crash loop cannot fill the volume.
- **Severity** is stormcast's rule, applied to the first 120 characters:
  - klog prefixes;
  - otherwise the leftmost of PANIC/FATAL, ERROR, WARN, DEBUG/TRACE, INFO;
  - a line with no level is a warning on stderr, info on stdout.
- **Events.** Process lifecycle, cron, backup and updater events are
  **always written to the log**. Optionally they are also POSTed as JSON to
  a webhook.

---

## Everything else it does today

- **Web console.** A Svelte SPA embedded in the binary at `/ui/` (no node
  at runtime):
  - dashboard cards, or a relational grid;
  - terminals and logs;
  - 12 themes;
  - plugin tabs, reverse-proxied for processes that have their own UI.
- **stormsh.** A TUI that renders **the same** `/api/v1/components` feed.
- **SSH.** A management shell with pipes, redirection and tab completion,
  plus SFTP. The password is the configured one or the cloud ID; public keys
  come from CloudID.
- **63 busybox-style applets** through `argv[0]` (`ls`, `curl`, `ping`, …),
  so a scratch container can be looked around in.
- **Also:** cron (6 fields), log backup (tar.gz POSTed on failure), an OCI
  image updater, login (users, bearer token), `--healthcheck`, zombie
  reaping.

---

## Interfaces

| | |
|---|---|
| **Config** | `/etc/stormd/config.toml` (`--config`): `[general] [api] [[process]] [[cron]] [events] [backup] [updater] [ssh] [debug] [stormlog.*]` |
| **REST** | `:9080/api/v1/…`: status, processes (start/stop/restart), logs, terminal, cron, updates, backup, plugins, `shutdown` |
| **WebSocket** | `/ws/console/{p}`, `/ws/logs`, `/ws/components` (full snapshot every 2 s) |
| **Health** | `GET /api/v1/health` → `{"status":"ok"}`, open, answered whenever the API is up. `stormd --healthcheck` calls it |
| **Metrics** | `GET /metrics`, Prometheus, open: `stormd_up`, `stormd_process_state`, `_restarts_total`, `_crashes_total`, `_uptime_seconds`, `process_resident_memory_bytes`, … |
| **Out** | UDP `239.255.42.1:5514` (logs), a webhook, a backup URL, CloudID, registries |
| **Ports on a node** | fastetcd 9081 · rustkube 9082–9085 · service goldens: the service's port + 100 |

---

## How it ships and is operated

- **Not a golden of its own.** stormcentral lists it as `kind = "special"`.
  It is `/stormd` in every **stormdbase** golden:
  - stormcos `build-goldens.sh` stages `/stormd`, the applet links (relative
    targets) and `/var/log/stormd`;
  - it adds the component's binary and `/etc/stormd/config.toml`, with log
    limits sized to the 64 MiB log volume;
  - it seals a deterministic tar.
- **Start.** The container's argv is `/stormd`. It loads the config
  (exit 1 if invalid), starts the processes, and binds the API.
- **Update.** A stormd commit reaches a node only when a component golden
  pinning it is rebuilt and released. The authority for that is stormcos
  `docs/goldens.md`.
- **Operate.**
  - Web console at `http://<node>:<port>/ui/`.
  - `stormsh -H <node> -p <port>`.
  - `curl …/metrics`.
  - Logs come off the multicast group through mcastsyslog.
- **Build.** `sc-build` on dev.g8.lo (static musl). `web/dist` is committed.

---

## Status — v0.7.0

- **Shipped.** Supervision with ready and liveness probes, the component feed
  and both dashboards, login, themes, the stormview UI system, stormcast log
  wire, and non-retryable exit codes (#2).
- **Docs** were rewritten from the code (#5). `config/example.toml` is now
  covered by a test.
- **Open issues that matter:**
  - **#9** — stop, restart and shutdown are **SIGKILL**, with no SIGTERM or
    grace period.
  - **#12** — container logs reach the multicast group **without stormcast's
    limiter**; a looping process floods it.
  - **#8** — the updater does not start an image process whose rootfs
    already exists.
  - **#11** — an unknown applet name (`/bin/ps` in goldens) starts a second
    init.
  - **#3** — refuse to spawn with an unexpanded `${NODE_IP}`.
  - **#1** — the log writer should create `log_dir` on demand and rate-limit
    open failures.
  - **#7, #10** — config keys that do nothing, cron timeouts that don't kill,
    the liveness counter resetting, the proxy dropping headers.

---

## Planned — not in the code yet

From the open issues. **None of this works today:**

- Graceful stop: SIGTERM, a per-process timeout, then SIGKILL, with shutdown
  going in reverse dependency order (#9).
- Rate limiting and repeat-collapsing, per process, on the multicast group
  (#12).
- Refusing to start as init under a name that is not an applet (#11), and
  refusing an unexpanded `${NODE_IP}` (#3).
- Starting image processes from an existing rootfs after a restart (#8).
- Warnings for unknown config keys, and dead keys removed or implemented (#7).
- A real liveness counter, a cron timeout that kills, and a header-preserving
  plugin proxy (#10).
- The log writer creating `log_dir` on demand and rate-limiting open
  failures (#1).

---

## Where to look

| | |
|---|---|
| Full reference: every config key, endpoint and metric | `README.md` |
| Plugin tabs and card summaries | `docs/plugin-ui.md` |
| Every config key, parse-tested | `config/example.toml` |
| Supervision | `crates/stormd/src/supervisor.rs` |
| Log path | `crates/stormlog/src/lib.rs`, stormcast |
| How goldens are built | stormcos `docs/goldens.md` |
| Work plan and history | `CLAUDE.md`, `CHANGELOG.md` |
