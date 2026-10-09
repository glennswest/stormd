# Changelog

## [Unreleased]
<!-- New unreleased changes go here -->

### 2026-10-08
- **feat:** when a process exits with an error, stormd writes the last 20
  lines of that run's output (stdout and stderr) to its own stderr as
  `name| line`, just before `process exited with error` (#29). stormd's
  output is all a node keeps of a container, so the cause (fastetcd's `DB
  corrupted`) now reaches the console, stormpump.log and assets.json.
- **perf:** a dependent starts the moment its dependency is ready or its
  one-shot has finished, instead of on the next 250 ms poll (#25). State and
  readiness changes wake the wait, with a 1 s poll as a backstop. A
  dependency's `readiness_probe`/`startup_probe` (#48) now counts, as
  `ready_probe` did.
- **fix:** a cron job that passes its `timeout_secs` is killed, not left
  running (#10). Each job runs on its own task, so a long one no longer
  holds up the others, and a job still running skips its next fire time
  with a WARN.
- **fix:** `stormd_process_liveness_failures_total` is a real counter (every
  failure since start); the consecutive count is the new gauge
  `stormd_process_liveness_consecutive_failures` (#10).
- **fix:** `[stormlog.mcast] group = "off"` (or `""`) sends nothing (#27).
  It used to be replaced by the fleet's default group, so a container
  configured to be quiet emitted anyway. A group that does not parse sends
  nothing, with a WARN.
- **fix:** a CloudID key refresh answered 404 says why it probably happened:
  on a stormcos node the link-local address is stormimds, not cloudid
  (#30). The rest of #30 waits on stormimds#9.
- **fix:** stormcast bumped 0.1.0 (`9244121`) → 0.1.1 (`3cec734`) (#28). The
  old pin panicked PID 1 when a line's byte 8192 fell inside a multibyte
  character (stormcast#4). The bump also brings stormcast#5: HOSTNAME and
  APP-NAME are one RFC 5424 token each (a space or non-ASCII becomes `_`),
  and IPv6 groups deliver.
- **fix:** under a process-group signal (`timeout`, Ctrl-C), a child that
  died of the same SIGTERM/SIGINT is logged as a stop, not as a crash with a
  restart scheduled (#26). The signal path starts shutdown first thing, and
  a signal death waits up to 300 ms for it before being counted.
- **feat:** Kubernetes-style `startup_probe`, `liveness_probe` and
  `readiness_probe` per process (#48): `http_get` / `tcp_socket` / `exec`,
  `initial_delay_seconds`, `period_seconds`, `timeout_seconds`,
  `success_threshold`, `failure_threshold`, Kubernetes defaults and
  spellings. The startup probe gates the others, so a slow start is never
  killed. Liveness failing kills the run (SIGTERM, then SIGKILL) for the
  restart policy. Readiness only marks the process ready or not.
- **feat:** `restart_policy = "Always" | "OnFailure" | "Never"` (#48), as in
  Kubernetes. Each restart backs off exponentially (10 s doubling, capped at
  5 min, reset after 10 minutes of running), in a new `CrashLoopBackOff`
  state, with no restart limit. When set, it replaces
  `on_exit`/`on_failure`/`restart_delay_secs`/`max_restarts`.
- **feat:** Kubernetes-shaped events for every probe failure, kill, back-off
  and start (`Unhealthy`, `Killing`, `BackOff`, `Created`, `Started`, with
  upstream's wording, de-duplicated with `count`), served at `GET
  /api/v1/events?since=<seq>` for rustkube-node to put on mirror pods (#48).
- **BREAKING:** the old `[process.liveness]` no longer kills anything (#48).
  It parses, warns that it is retired, and is ignored. On the Dell it killed
  the apiserver six times during a normal 40 s start; components move to
  the new probes. `grpc` probes speak `grpc.health.v1.Health/Check` over
  plaintext HTTP/2 (reqwest gains its `http2` feature).
- **feat:** `POST …/restart?wait=healthy&timeout=N` answers once the new run
  is healthy (200), it ended (502), or the timeout passed (504, process left
  running, with what it still waits on) (#44, stormcos#25). Healthy means
  every check the process has passed since that spawn: ready_probe,
  liveness, declared APIs; with none, 3 s running. Process status adds
  `run`, `ready`/`ready_at`, `liveness_passed_at`/`_run` and `healthy`.
- **fix:** a restarted process's `ready_probe` is watched again, once per
  run, so a restart becomes `ready` again (#46).
- **feat:** API health probes (#49, stormcos#458). `[[process.api]]`
  declares a process's real APIs (url, interval, timeout, `p50_ms`/`p99_ms`
  budgets, bearer token or client cert). stormd times them into `healthy`,
  `slow`, `stalled` or `down`. Each change is logged loudly with the latency,
  the state before and how long it lasted, and appended to
  `/system-data/history/api/<process>.jsonl` when that volume is there.
  `GET /api/v1/health/apis` serves the current state. Opt-in
  `restart_after_stalled_secs` restarts the process through the restart
  policy, logged as such.

### 2026-10-07
- **test:** the test image follows the updated standard (#24). It is one
  image for all suites, built from `test/Containerfile` with the repo root
  as context. `test/build.sh` only builds the binaries into `test/out/` (no
  podman), the test program takes its suite from `/test <suite>` (falling
  back to `STORM_SUITE`), and the Job names that command.
- **docs:** stormd ships as an input golden (`kind = "input"`), recorded
  after each issue that passes sc-build. This replaces "stormd never
  requests goldens" (owner, #43). The first one is
  `golden-stormd-3e9d395470bf` (36a95f0). README and presentation no longer
  call it `kind = "special"` (#35).
- **fix:** standalone applets exit non-zero when they fail (#31): 1 on
  failure (`stat`/`cat`/`ls` of a missing file, `grep` with no match, `ping`
  with no answer, …), 2 on bad usage. Before, everything but `false` exited
  0, so no one-shot or exec probe could wait for a file.
- **fix:** a standalone applet reads stdin only if it takes input and was
  given no file (as coreutils). Before, every applet read all of stdin
  first, so under stormd, whose child stdin is a pipe that never closes,
  `/bin/test -e …` as a one-shot or `cat FILE` hung forever (#31).
- **feat:** a `test` / `[` applet (`-e -f -d -s -r -w -x`, `-n`, `-z`, `=`,
  `!=`, `!`) for exactly that (#31, stormcos#81). Goldens link it through
  `--list-commands`.

## [v0.8.0] — 2026-10-07 (stormd 0.8.0 · stormlog 0.4.0 · stormsh 0.5.0)

### Breaking
- with auth on, `/metrics` now needs credentials like every
  other data route (bearer token or client certificate); scrapers need the
  token (stormcos#64).

### Added
- `/metrics` reports each running supervised process's own RSS,
  virtual memory, CPU seconds and open fds (`stormd_process_resident_memory_bytes`,
  `_virtual_memory_bytes`, `_cpu_seconds_total`, `_open_fds`, labelled
  `{container,process}`) (#33). Before, only stormd's own memory was there,
  so a leak in the supervised binary was invisible.
- goldens for a process (#36, minismbd#11 option A).
  `[[process.golden]]` names a stormblock golden (by name or volume id) as
  `filesystem` or `image`. Before the process first starts, stormd attaches
  each one read-only over ublk from the node's engine (`[goldens]
  engine_url`, `token_file`) and makes the device node. A filesystem golden
  is mounted read-only at its path; an image golden becomes a device node
  there with the given owner and mode. Failures are retried every 2 s; at
  shutdown everything is unmounted and detached. `GET /api/v1/goldens` lists
  what is presented, and `PUT /api/v1/processes/{p}/goldens/{name}` swaps
  one golden for another, restarting the process.
- `[process] wait_for_files` — a process's first start waits until
  every listed file exists (absolute paths, `${NODE_IP}`/`${NODE_NAME}`
  expanded, polled every 250 ms, one log line naming what is missing), with
  no restart counted and no cool-off (#38). For fastetcd, which crash-looped
  2–4 s on every boot until its minted cert existed (stormcos#300).
- the API can be served over TLS and closed to anonymous callers
  (#32). `[api] tls_cert_file`/`tls_key_file` (PEM, rustls, HTTP/1.1,
  re-read when either file changes so a rotated stormcert pair is picked up
  without a restart); `client_ca_file` — a client certificate that verifies
  against it authenticates the request; `token_file` — the bearer token read
  from a file and re-read on change. Either of the last two turns auth on. A
  bad pair or CA stops stormd at start, before anything is spawned. stormd
  warns at start when auth is off, or on without TLS. `/healthz` is a new
  alias of `/api/v1/health`.
- `stormd --healthcheck` falls back to https when the port speaks
  TLS (loopback, no credential, certificate not checked) (#32).
- stormsh: `--ca-file` (https, trusting only that CA),
  `--cert`/`--key` (client certificate) and `--token-file`, for a stormd
  behind TLS and auth (#32).
- `[process] env_default = { KEY = "value" }` — each entry is set
  only when KEY is not already in stormd's own environment, so a node's
  override (stormpump `env.d/<spec>`, stormcos#282) beats the golden's
  default; `env` still wins over both. Values get `${NODE_IP}`/`${NODE_NAME}`
  like `env` (#37).

### Fixed
- updater: an image process whose rootfs already exists is started
  when stormd starts (#8). Each pull and pivot records the image, digest,
  command, env and working directory in `<rootfs_dir>/<name>.image.json`,
  and a restart starts from that record. The record's digest (not the
  registry's) is the current one, so an image published while stormd was
  down is pulled on the first poll. An `image` process with the updater
  disabled gets an ERROR at start instead of silence.
- stormlog: a log file that cannot be opened no longer logs one
  ERROR per line (#1). A missing directory is created again and the open
  retried. A persistent failure is said once, retried at most every second,
  and repeated at most once a minute with the count of lines not written.
  On recovery the file gets one marker line for the gap.
- the API is bound before anything is started (#23). A taken port
  used to exit 1 after the start order, cron and the updater had begun,
  leaving their processes running unsupervised (a second fastetcd or
  apiserver). Now it exits 1 with nothing started. Medium suite:
  `api-port-taken`.
- stormlog: lines reach the fleet's multicast group through
  stormcast's limiter, one per process, as stormpump does on the host (#12).
  Repeats collapse to `last message repeated N time(s)`, and over 200 lines/s
  (after a 2000 burst) lines are dropped with a count. The run is flushed
  when a process's output ends, and stormd's crash marker is never dropped.
  The file and live streams keep every line.
- a process is never spawned with `${NODE_IP}`/`${NODE_NAME}` left
  unexpanded in its arguments or applied environment (#3). It waits, with
  one ERROR naming the process, the name and the reason (no address / no
  route; empty hostname), and starts when the node has the value. This
  applies at first start and before each restart. An API start is refused
  with the message. Before, a node without an address failed three layers
  down: stormcert-init exited 2 parsing `--ip …,${NODE_IP},…`, and the
  container was retried every 300 s.
- stop, restart, the updater's pivot and shutdown send SIGTERM,
  wait up to the process's new `stop_timeout_secs` (default 10; 0 = SIGKILL
  at once), then SIGKILL, instead of SIGKILL outright, and record the exit
  code (#9). Shutdown stops processes in reverse dependency order, a tier at
  a time, waiting for each. Restart and the updater wait for the old run to
  be gone instead of a fixed 500 ms / 5 s. The shutdown watchdog allows the
  processes' stop budget plus 20 s, never less than 30 s.
- a liveness task ends with its run, and `liveness_failures` resets
  at every spawn (#45, P0). A task asleep in `initial_delay_secs` across a
  crash and restart woke on the new run and probed it at once with the old
  count, and each restart added one more: once a slow start tripped the
  threshold, every later run was SIGUSR1'd seconds after it started
  (fastetcd and the apiserver crash-looping on the Dell). Now one task per
  run, aborted when the run ends; every check, count and signal applies only
  to that run, and the signal goes to that run's pid.
- stormd under a name that is neither `stormd` (or a renamed copy,
  `stormd-*`/`stormd.*`) nor an applet — `/bin/ps` linked to it, say — prints
  `stormd: <name>: not a stormd applet (see stormd --list-commands)` and exits
  127, instead of starting a full init on the default config that spawned a
  second copy of every supervised process (#11). Medium suite:
  `unknown-argv0-refused`.
- the plugin proxy (`/ui/proxy/{name}/…`) forwards the request's
  headers (minus hop-by-hop ones, `Host`, `Content-Length`) and its body as
  bytes, passes any method through, and returns the plugin's response headers
  (`Set-Cookie`, `Location`, caching…) instead of only `Content-Type`.
  Redirects go back to the browser instead of being followed, and one client
  is reused. A plugin's own `Authorization` now reaches it, so stormstorage
  UI writes work with its `api_token` set (#34). stormd's own credentials
  (its `auth_token` bearer, the `stormd_session` cookie) are not passed on,
  and a plugin cannot set `stormd_session`.

### Changed
- test-fixture credentials marked `not a secret` (inline, or `.github/secret_scanning.yml` for files that cannot hold a comment) — owner

### Documentation
- README and presentation say what #31 and #32 record: every
  standalone applet but `false` (1) and unknown names (127) exits 0 even on
  error, so an exec probe or one-shot built on an applet cannot fail; the API
  is plain HTTP only and anonymous unless `auth_token`/`password`/a user is set.
- third check of the docs against the code (no code change since
  v0.7.4): every route, config default, port, the 63 applets, the log-volume/PVC
  wording and how-it-ships re-checked against the source; README and
  presentation already describe #1, #3, #7–#12 and #23–#30. Nothing to correct.
- refreshed from the code at v0.7.4. README: one-shot dependencies,
  bounded shutdown and IMDSv2 in the overview. Startup order corrected: SSH
  starts before the API binds, and a failed SSH bind is only logged. A failed
  API bind leaves spawned processes running (#23). The shutdown crash line
  under process-group signals (#26), the 250 ms dependency poll (#25), and
  cron jobs running one at a time with `next_run`. How it ships: goldens
  build stormd at main, stormcos rebuilds them at release, stormd never
  requests one, and the stormdbase applet list links `ps` (#11). The test
  container predates the runner's contract (#24). The presentation is
  updated to v0.7.4 (shipped since v0.7.0, open and planned issues). The
  shutdown design note records #17's bounded shutdown. CLAUDE.md: the
  workspace's test member, goldens at main, binary size
- Second refresh pass. There was no code change since the first; defaults,
  routes, applets and validation were re-checked against the source. The README now says
  what the issues filed since record: `[stormlog.mcast] group = "off"` still
  sends (#27), no limiter on the group (#12), the pinned stormcast's multibyte
  truncation panic (#28), a failed process's error not reaching stormd's own
  output (#29), the per-line ERROR on a failed log open (#1), an unexpanded
  `${NODE_IP}` spawned anyway (#3), and CloudID on a stormcos node reaching
  stormimds (#30). The log volume is named as stormcos names it (the component's
  `-logs` golden clone). Outside a golden, `log_dir` belongs on a PVC (on
  stormcos, the built-in stormblock driver). `config/example.toml` comments
  and the presentation's status and planned slides are updated to match.

## [v0.7.4] — 2026-09-26 (stormd)

### Fixed
- #22 exits were handled one at a time, and a restart's cooloff
  (up to 30 s) was slept inside that handling — so while one process cooled
  off, every other process that died stayed `running`, unrestarted. Each exit
  is now handled on its own task. Found by the #15 long suite (waves took
  ~100 ms per process to settle)
- #19 the CloudID SSH-key refresh sent bare GETs, which stormimds
  (default `security.mode = "both"`) answers 401 with an empty body — parsed
  as an index with no entries: no keys, no warning. It now speaks IMDSv2
  (`PUT /latest/api/token`, then `X-aws-ec2-metadata-token` on each GET,
  falling back to no token when none is issued) and sends
  `Metadata-Flavor: StormIMDS`; a non-2xx answer is an error with its
  status; a stale token is replaced once; a persistent failure is warned
  about once, not every 30 s

## [v0.7.3] — 2026-09-26 (stormd)

### Fixed
- #20 a compile error in the new test crate (9205cd5, fixed in
  265acc1) broke every service golden's `build stormd` step, because the
  golden build compiles the whole workspace. `test/` is now out of the
  workspace's `default-members`: a bare `cargo build` no longer compiles it
- #21 no `[[cron]]` job ever ran: the scheduler ran a job when
  `upcoming().next()` was not in the future, which it never is. Each job now
  keeps its next fire time and advances it when it comes; `GET /api/v1/cron`
  reports that time. Found by the #15 medium suite

### Added
- #15 the test container, `stormd-test-<suite>`, per stormcentral's
  test standard: `test/` (workspace member `stormd-test`), `test/build.sh`,
  `test/Containerfile` (scratch: `/stormd` of the commit + `/test`),
  `test/stormd-test.yaml` (the Job). It runs the stormd under test as its
  child and drives it through the REST API; `short` (start order, ready
  probe, one-shot, crash restart, logs, SIGTERM with nothing left, the node's
  stormds' health), `medium` (failure paths and features end to end), `long`
  (waves sized from the pod's allowance, measured for slowdown and residue)

### Documentation
- README "Tests" section; README's stormd version was stale (0.7.0)

## [v0.7.2] — 2026-09-26 (stormd)

### Fixed
- #17 SIGTERM/SIGINT did not stop stormd when its start order was
  waiting on a `depends_on` that could never be satisfied (e.g. a dependent
  of a one-shot that failed): the signal was received, then shutdown waited
  for the start order forever — ten hours under `timeout 10` on dev, holding a
  build slot. Shutdown now sets a flag that ends the start order and any
  dependency wait, stands down pending restarts and refuses new starts;
  `stop_all` waits up to 10 s for the kills to land and runs again once the
  start order has ended; and a watchdog thread exits stormd 30 s after
  shutdown began if anything else stalls
- #17 a process whose exit is handled after shutdown began is recorded as
  stopped — not a crash, no restart. (Under `timeout`, which signals the whole
  process group, the child can die before stormd starts shutting down; that
  exit is still logged as a crash and a restart scheduled, which then stands
  down.)

## [v0.7.1] — 2026-09-26 (stormd)

### Fixed
- #16 a `depends_on` naming a one-shot (`on_exit = "stop"`) with no
  `ready_probe` was satisfied the moment the one-shot spawned — no probe
  means ready at spawn — so dependents ran while it was still working
  (stormcos#60: stormcert-node-admin failed before stormcert-sa wrote its
  key). Such a one-shot now satisfies only once stopped after exiting 0; one
  with a probe also satisfies on the probe. A one-shot that failed or was
  stopped by hand no longer satisfies (it did when `on_failure = "ignore"`
  left it stopped); the held dependent logs one WARN naming it

### 2026-09-24
- **docs:** #6 `docs/presentation.md` — a 12-slide Marp deck: purpose,
  place in stormcos (stormcentral's relationships graph), moving parts,
  features today, interfaces, how it ships, status, and planned work (open
  issues, marked as not in the code); linked from the README
- **fix:** #5 `config/example.toml` did not load — it still said
  `transport = "nats"` (NATS was removed; only `none | webhook` parse) and
  carried keys stormd ignores. Rewritten from `config.rs`, and a unit test
  now parses and validates it so it cannot drift again
- **docs:** #5 README rewritten from the code: every config key with its
  real default (several were wrong, e.g. `max_restarts` is 10 not 100), keys
  that are parsed but do nothing marked as such, ready probes, the restart
  cooloff, `${NODE_IP}`, `/metrics`, ports, auth, the SSH exec/scp limits, and
  that stop/restart/shutdown are SIGKILL. NATS, MinIO and the stale stormdbase
  project list are gone. New "How it ships" section — stormd is `/stormd` in
  every stormdbase golden, with stormcos `docs/goldens.md` as the authority
  (#4). Building is `sc-build` on dev
- **docs:** #5 plugin UI guide moved to `docs/plugin-ui.md`, rewritten from
  the proxy code (what it forwards and what it does not); the Dracula style
  guide replaced by stormview's tokens. `enhancements/` → `docs/design/`, the
  shutdown note marked implemented except its SIGTERM step
- **docs:** #5 CLAUDE.md — build via `sc-build` (not root@dev / the Mac),
  how stormd ships, docs/ and nodevars in the key directories, DHCP notes
  pointed at the cross-project reference (mkube is retired)
- **docs:** #5 stale code comments — MinIO archival (gone; a run's file is
  renamed and pruned on the volume), and the conditions that turn auth on

## [v0.7.0] — 2026-08-30

### 2026-08-30
- **feat:** #2 non-retryable exits — `[[process]] no_restart_exit_codes`
  lists exit codes the process uses to say a restart will not help (sysexits
  78 EX_CONFIG, 64 EX_USAGE; stormconsole exits 78). Such an exit is not
  restarted, does not count toward `max_restarts`, logs one error line and
  marks the process failed; `on_no_restart = "hold" | "fail"` (default
  `hold`) decides whether the container keeps running or fails. Empty by
  default, so existing configs are unchanged. The ProcessCrashed event now
  carries `code` and, when applicable, `no_restart`
- **docs:** README process failure policies and config reference,
  example.toml

## [v0.6.0] — 2026-08-26

### 2026-08-26
- **feat:** named users — `[[api.users]]` entries (name + password) behind
  the login screen, which now asks user + password; the signed-in user shows
  in the nav and on the session endpoint. `[api] password` remains as the
  legacy "admin" user, and the bearer token still doubles as admin's login.
  Credential checks compare every configured pair in constant time, so
  timing reveals neither passwords nor which usernames exist.
- **feat:** four new themes from stormview v0.4.0 — One (One Dark) and
  Gruvbox dark, Frost (cool nordic) and Paper (warm sepia) light — twelve
  total, and Catppuccin's brand color is peach instead of the mauve that
  read as washed-out purple.
- **feat:** `[general] theme` sets the instance's default web UI theme,
  served on the open session endpoint (with the container name, so the
  login screen is branded and themed before auth). A viewer's own pick,
  stored in their browser, wins over the default.
- **feat:** two new low-eye-strain themes — Catppuccin Mocha and Rosé Pine —
  and the Storm default is rebased on Tokyo Night: low-glare indigo ground
  and softened accents instead of Dracula neon on near-black.
- **refactor:** the login screen is stormview's `LoginPanel` — redesigned
  (glyph, gradient thread, focus ring, inline error with shake) and
  reusable by stormconsole.
- **refactor:** the whole UI system moved to the stormview repo as an npm
  package — themes.css (all tokens + six themes), DataGrid, ComponentCard,
  ComponentGrid, RelationPicker, HealthDot, theme state, and the shared
  formatting/ANSI helpers. The components are app-agnostic (hosts inject
  `resolve`/`invoke`; navigation is hash hrefs); stormd's `web/` keeps only
  the app shell: routing, stores, auth, views. stormview is now both a Rust
  crate (the contract) and an npm package (the renderer) in one repo.
- **feat:** cards link to grids — a ⊞ on any card with `has_many` relations
  opens `#/grid?id=…` rooted at that component, and each relationship row
  has its own ⊞ (`&rel=…`) opening just that relationship's targets as the
  top of a nested grid.
- **feat:** the container name in the nav is a home link.
- **fix:** theme polish from review — Phosphor gets a taller brightness
  ladder so group boundaries actually show; Solar's washed-out foregrounds
  run brighter; Nord gets a darker ground and hotter aurora accents instead
  of uniform gray; Midnight trades white-on-black glare for parchment-gray
  text and stronger panel separation; Storm's ground drops a step below the
  panels. Globally: dashboard section headers gained a hairline rule, cards
  gained shadows and wider gaps — isolation and spacing, per theme review.

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

