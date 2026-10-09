---
marp: true
theme: default
paginate: true
title: stormd — the init inside every stormcos component
description: Purpose and functionality of stormd v0.8.0, from the code
---

<!-- Render: npx @marp-team/marp-cli docs/presentation.md          (HTML)
             npx @marp-team/marp-cli --pdf docs/presentation.md    (PDF)
     Written 2026-09-24 against stormd v0.7.0; refreshed 2026-10-09 for v0.8.0. Every claim is checkable in
     the source; README.md has the full reference. -->

# stormd

### The init inside every stormcos component container

v0.8.0 · github.com/glennswest/stormd

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
  has passed, or until a one-shot dependency (`on_exit = "stop"`) has
  exited 0 — running is not done.
- **Restart policy.** `on_failure` is `restart`, `fail` or `ignore`.
  `on_exit` is `restart` or `stop`. `max_restarts` counts restarts within
  `restart_window_secs`.
- **Back-off.** The delay is `restart_delay_secs × 2^(n-1)`, capped at
  30 s, so a process that can never start is retried every 30 s, not every
  second.
- **Non-retryable exits.** `no_restart_exit_codes = [78]` marks the process
  failed and does not restart it. By default it leaves the container
  running (`on_no_restart = "hold"`).
- **Probes, the Kubernetes way (#48).** `startup_probe`, `liveness_probe`,
  `readiness_probe` with `http_get`/`tcp_socket`/`exec`/`grpc` and
  Kubernetes' fields and defaults. Startup gates the others; liveness kills
  the run (SIGTERM, then SIGKILL); readiness only marks not ready. Per run,
  ending with it (#45). The old `[process.liveness]` is retired.
- **`restart_policy`** `Always`/`OnFailure`/`Never`: back-off 10 s doubling
  to 5 min, `CrashLoopBackOff`, reset after 10 min of running.
- **Events.** `Created`/`Started`/`Unhealthy`/`Killing`/`BackOff`, deduped
  as the kubelet does, at `GET /api/v1/events` for rustkube-node's mirror pod.
- **API health (#49).** `[[process.api]]`: a real read timed against p50/p99
  budgets → `healthy`/`slow`/`stalled`/`down`, each change logged once;
  restart on a long stall only when asked.
- **Restart that waits** — `restart?wait=healthy` answers when the new run
  has passed its checks (#44).
- **Goldens (#36).** `[[process.golden]]`: stormblock goldens attached
  read-only over ublk and mounted (or placed as a device node) before start;
  swapped at runtime by `PUT`.
- **`${NODE_IP}` / `${NODE_NAME}`** in args and env are filled in at every
  spawn, so a control plane advertises an address other nodes can reach.
- **`env_default`** — set only when stormd did not inherit the key, so a
  node's env.d overrides the golden's default; `env` wins over both.
- **`wait_for_files`** — first start only once named files exist (a minted
  cert pair), no restart or cool-off counted (#38).
- **Shutdown** (SIGTERM, SIGINT, API, container failure) stops the start
  order, stands restarts down, and stops every process, dependents first:
  SIGTERM, `stop_timeout_secs` (default 10), then SIGKILL (#9). Bounded
  whatever stalls. Each exit is handled on its own task.
- **A failed run's last 20 lines** are echoed on stormd's own stderr, so the
  node's console shows why it failed (#29).

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
  come from CloudID over IMDSv2 (keys arrive once stormimds serves
  `public-keys/`, stormimds#5).
- **65 busybox-style applets** through `argv[0]` (`ls`, `curl`, `ping`, …),
  so a scratch container can be looked around in.
- **Also:** cron (6 fields), log backup (tar.gz POSTed on failure), an OCI
  image updater, login (users, bearer token), `--healthcheck`, zombie
  reaping.

---

## Interfaces

| | |
|---|---|
| **Config** | `/etc/stormd/config.toml` (`--config`): `[general] [api] [[process]] [[cron]] [events] [backup] [updater] [ssh] [debug] [goldens] [stormlog.*]`; an unknown key is one WARN, never a refusal (#7) |
| **REST** | `:9080/api/v1/…`: status, processes (start/stop/restart, `?wait=healthy`), events, `health/apis`, goldens, logs, terminal, cron, updates, backup, plugins, `shutdown` |
| **WebSocket** | `/ws/console/{p}`, `/ws/logs`, `/ws/components` (full snapshot every 2 s) |
| **Health** | `GET /api/v1/health` / `/healthz` → `{"status":"ok"}`, open, answered whenever the API is up. `stormd --healthcheck` calls it |
| **Auth/TLS** | `[api] tls_cert_file`/`tls_key_file` (re-read on rotation), `client_ca_file` (client certs), `token_file` (bearer); with any credential, nothing but health is anonymous (#32) |
| **Metrics** | `GET /metrics`, Prometheus, behind the same auth: `stormd_up`, `stormd_process_state`, `_restarts_total`, `_crashes_total`, `_uptime_seconds`, `_liveness_failures_total`, each process's RSS/CPU/fds (#33), … |
| **Out** | UDP `239.255.42.1:5514` (logs), a webhook, a backup URL, CloudID, registries |
| **Ports on a node** | fastetcd 9081 · rustkube 9082–9085 · service goldens: the service's port + 100 |

---

## How it ships and is operated

- **An input golden.** stormcentral lists it as `kind = "input"`, and the
  platform hands its binary to every **stormdbase** golden as `/stormd`:
  - stormcos `build-goldens.sh` stages `/stormd`, the applet links (relative
    targets) and `/var/log/stormd`;
  - it adds the component's binary and `/etc/stormd/config.toml`, with log
    limits sized to the 64 MiB log volume;
  - it seals a deterministic tar.
- **Start.** The container's argv is `/stormd`. It loads the config
  (exit 1 if invalid), binds the API (a taken port exits 1 before anything
  starts — #23), and starts the processes.
- **Update.** stormd is an input golden, recorded after each issue that
  passes sc-build (#43). The goldens built on it pick it up, and stormcos
  releases them. The authority is stormcos `docs/goldens.md`.
- **Operate.**
  - Web console at `http(s)://<node>:<port>/ui/` (https once the config sets TLS).
  - `stormsh -H <node> -p <port>`.
  - `curl -H "Authorization: Bearer …" …/metrics` (with auth on).
  - Logs come off the multicast group through mcastsyslog.
- **Build.** `sc-build` on dev.g8.lo (static musl). `web/dist` is committed.

---

## Status — v0.8.0

- **Shipped.** Supervision with probes, the component feed and both
  dashboards, login, themes, the stormview UI system, stormcast log wire,
  non-retryable exit codes (#2).
- **v0.8.0 (2026-10-07):** API over TLS, no anonymous access (#32);
  SIGTERM-first stop, dependents first (#9); `env_default` (#37);
  `wait_for_files` (#38); goldens (#36); per-process metrics (#33); limiter
  on the group (#12); API bound before anything starts (#23); no unexpanded
  `${NODE_IP}` (#3); updater starts from an existing rootfs (#8).
- **Since v0.8.0 (on main, in the input golden):** Kubernetes probes,
  restart policy and events (#48); API health (#49); restart that waits for
  health (#44); applet exit codes and `test` (#31); cron timeout kills (#10);
  last words on stderr (#29); unknown config keys warned (#7).
- **Docs** refreshed from the code 2026-10-09. `config/example.toml` is
  covered by a test.
- **Open issues that matter:**
  - **#50** (P0) — pin the git dependencies to a rev.
  - **#52** — API health to a state file for PID 1.
  - **#30** — on a node, CloudID's address is stormimds, which does not know
    the node (decision pending in stormimds#9).

---

## Planned — not in the code yet

From the open issues. **None of this works today:**

- Git dependencies pinned to a rev (#50).
- API health written to a state file (#52), and what was in flight on a
  stall (#54).
- Plugin proxy: WebSocket, absolute `Location` rewrite, body limit (#53).
- Shell/dashboard liveness from `liveness_probe`, not the retired key (#56).
- An exit under a group signal always logged before stormd exits (#55).

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
