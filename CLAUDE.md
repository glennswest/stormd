# CLAUDE.md — Project Instructions

## Core Rules

1. **All changes are approved.** Do not ask for confirmation before making changes. Execute the work.
2. **Every change must be committed to GitHub.** No uncommitted work. Commit early, commit often. Use clear, descriptive commit messages following conventional commits format (e.g., `feat:`, `fix:`, `refactor:`, `docs:`, `chore:`).
3. **Push after every logical unit of work.** Do not batch large numbers of changes into a single push.
4. **Commit first, test after.** Get the work saved and pushed before running tests. If tests fail, fix and commit the fix as a separate commit. Never leave working code uncommitted while chasing test failures.
5. **A changelog must be maintained.** Every change, no matter how small, must be logged in `CHANGELOG.md` with date, description, and category.
6. **Documentation must stay current.** If you change behavior, update the relevant docs immediately — not later, not in a follow-up. Code and docs ship together.
7. **This file (`CLAUDE.md`) is the work plan.** Update the task lists below as you progress. Check off completed items. Add new items as they emerge.
8. **No sensitive information in commits.** Scan every change for secrets before committing. Maintain `.gitignore` proactively.
9. **Preserve context at all times.** Assume a power loss or disconnection can happen at any moment. Commit and push frequently so no context or work is ever lost.
10. **Follow semantic versioning.** Bump versions according to the rules below. Version bumps are their own commit.

---

## Version Management

This project follows [Semantic Versioning 2.0.0](https://semver.org/) — `MAJOR.MINOR.PATCH` (e.g., `1.4.2`).

### When to Bump Versions

| Change Type | Version Bump | Examples |
|---|---|---|
| **Breaking changes** — API removals, behavior changes that break consumers, config format changes, renamed public interfaces | **MAJOR** (`X.0.0`) | Removing a CLI flag, changing a function signature, altering default behavior |
| **New features** — Backward-compatible additions, new endpoints, new CLI commands, new config options | **MINOR** (`x.Y.0`) | Adding a new module, new optional parameter, new command |
| **Bug fixes** — Backward-compatible fixes, typo corrections in behavior, performance improvements | **PATCH** (`x.y.Z`) | Fixing a crash, correcting a calculation, patching a security issue |

### Pre-1.0 Rules
- While the project is in initial development (`0.x.y`), the API is not considered stable.
- **MINOR** bumps (`0.X.0`) may include breaking changes during pre-1.0 development.
- **PATCH** bumps (`0.x.Y`) are still bug fixes only.
- The `1.0.0` release signals the public API is stable and the full semver contract applies from that point forward.

### Version Bump Workflow
1. **Determine the bump type** based on the changes since the last version.
2. **Update the version number** in all locations where it is defined (see "Version Locations" below).
3. **Update `CHANGELOG.md`** — move the `[Unreleased]` section contents under the new version heading.
4. **Commit the version bump separately** with message: `chore(release): vX.Y.Z`
5. **Tag the commit**: `git tag vX.Y.Z`
6. **Push the tag**: `git push origin vX.Y.Z`

### Version Locations
Update the version in **all** of these locations (project-specific — fill in as applicable):

```
# Examples — replace with actual paths for this project:
# Cargo.toml         → version = "X.Y.Z"
# package.json       → "version": "X.Y.Z"
# pyproject.toml     → version = "X.Y.Z"
# VERSION file        → X.Y.Z
# src/lib.rs          → pub const VERSION: &str = "X.Y.Z";
# README.md badges   → version shield URL
```

**All version locations must match.** If you update one, update all. A version mismatch across files is a bug — fix it immediately.

### When to Release

- **PATCH releases** — After any bug fix or set of related bug fixes. Can be frequent.
- **MINOR releases** — When a new feature or meaningful enhancement is complete and tested. Group related features if they land close together.
- **MAJOR releases** — Deliberate decision. Document the breaking changes thoroughly in the changelog. Never bump MAJOR as a surprise — log it in the Major Changes section of the work plan first.

### Changelog Integration for Releases

When cutting a release, transform the changelog:

```markdown
# Changelog

## [vX.Y.Z] — YYYY-MM-DD

### Added
- Feature descriptions (from feat: entries)

### Fixed
- Bug fix descriptions (from fix: entries)

### Changed
- Refactor or behavior change descriptions (from refactor:/perf: entries)

### Breaking
- Breaking change descriptions (from BREAKING: entries)

### Documentation
- Doc update descriptions (from docs: entries)

## [Unreleased]
<!-- New unreleased changes go here -->
```

---

## Context Preservation — Anti-Loss Protocol

**Assume the connection can drop at any time.** Work must survive a sudden disconnect, power failure, or session timeout.

### Rules
- **Commit and push after every meaningful change** — not at the end of a session, not when "done," but continuously as you work.
- **Update `CLAUDE.md` work plan before starting new tasks** — if the session dies mid-task, the next session must know what was in progress, what was completed, and what's next.
- **Write intentions before executing.** Before starting a multi-step change, update the work plan below with what you're about to do. Commit that update. Then do the work.
- **Never hold state only in memory.** If you've figured something out, learned something about the codebase, or made a decision — write it down in `CLAUDE.md` or relevant docs and commit it immediately.
- **Work in small increments.** Prefer 5 small commits over 1 large commit. Each commit should be a recoverable checkpoint.
- **If in doubt, commit what you have.** A partial commit with a `WIP:` prefix is better than lost work. Follow up with a clean commit when complete.

### On Resume After Disconnect
- Read `CLAUDE.md` first to understand current state
- Check `CHANGELOG.md` for recent activity
- Run `git status` and `git log --oneline -10` to understand where things left off
- Check `git tag --sort=-v:refname | head -5` to see current version
- Continue from where the work plan indicates

---

## Sensitive Information & Security

### Before Every Commit — Mandatory Scan
Before staging and committing, check all changed files for:
- **API keys, tokens, secrets** (any string resembling a key or token)
- **Passwords and credentials** (hardcoded or in config files)
- **Private keys** (SSH, TLS, PGP, etc.)
- **Connection strings** with embedded credentials
- **Internal hostnames, IPs, or infrastructure details** that shouldn't be public
- **Personal information** (email addresses, phone numbers, physical addresses unless intentional)
- **Environment-specific paths** that reveal system structure

### If Sensitive Information Is Found
1. **Do not commit the file.** Remove or redact the sensitive data first.
2. Move secrets to environment variables, `.env` files, or a secrets manager.
3. Add appropriate entries to `.gitignore`.
4. If secrets were accidentally committed in a previous commit, flag it immediately in the work plan — this requires history rewriting or key rotation.

### .gitignore Maintenance
- **`.gitignore` must be kept current.** When adding new tools, dependencies, build artifacts, or config files that contain secrets, update `.gitignore` in the same commit.
- Common entries to always include:
  ```
  # Secrets and environment
  .env
  .env.*
  *.pem
  *.key
  *.p12
  *.pfx
  secrets/
  credentials/

  # IDE and OS
  .vscode/
  .idea/
  *.swp
  *.swo
  .DS_Store
  Thumbs.db

  # Build artifacts
  target/
  dist/
  build/
  node_modules/
  __pycache__/
  *.pyc

  # Logs and temp
  *.log
  tmp/
  temp/
  ```
- When introducing a new file type or directory that should be ignored, add it to `.gitignore` **before** the file is created, not after.
- Periodically verify nothing sensitive has slipped through: `git ls-files` to audit tracked files.

---

## Change Management

### Commit Standards
- One logical change per commit
- Commit message format: `type(scope): description`
- Types: `feat`, `fix`, `refactor`, `docs`, `test`, `chore`, `perf`, `build`
- Tag breaking changes with `BREAKING:` prefix in commit body
- Reference issue numbers when applicable
- Use `WIP:` prefix for partial work that needs to be saved immediately
- Version releases use: `chore(release): vX.Y.Z`

### Workflow Order
1. **Update work plan** in `CLAUDE.md` with what you're about to do → commit & push
2. **Make the change** → commit & push
3. **Update changelog** → commit & push (can combine with step 2 if small)
4. **Update documentation** → commit & push (can combine with step 2 if small)
5. **Run tests / linter** → if failures, fix → commit & push the fix
6. **Check off completed task** in work plan → commit & push
7. **If version bump is warranted** → bump version, update changelog heading, tag, push

### Before Every Change
- Review existing code and tests in the affected area
- Ensure you understand the current behavior before modifying it
- Scan for sensitive information in files you're about to modify

### After Every Change
- Scan diff for sensitive information (`git diff --staged`)
- Verify `.gitignore` covers any new artifact types
- Update `CHANGELOG.md`
- Update any affected documentation (README, inline docs, API docs)
- Commit and push
- Evaluate whether a version bump is needed

---

## Changelog Format (`CHANGELOG.md`)

Maintain `CHANGELOG.md` in the project root using this format:

```markdown
# Changelog

## [Unreleased]

### YYYY-MM-DD
- **feat:** Description of feature added
- **fix:** Description of bug fixed
- **refactor:** Description of refactor
- **docs:** Description of documentation update
- **chore:** Description of maintenance task
- **perf:** Description of performance improvement
- **BREAKING:** Description of breaking change

## [vX.Y.Z] — YYYY-MM-DD

### Added
- ...

### Fixed
- ...

### Changed
- ...

### Breaking
- ...
```

---

## Documentation Requirements

- `README.md` — Must reflect current project state, setup instructions, and usage
- Inline code comments — Update or add when logic is non-obvious
- API/interface docs — Update when signatures, behaviors, or contracts change
- Configuration docs — Update when config options change
- Architecture docs — Update when structural changes are made

If a documentation file doesn't exist yet and should, create it.

---

## Work Plan

### Current Version: stormd `v0.8.0` · stormsh `v0.5.0` · stormlog `v0.4.0` · stormview `v0.4.0` (own repo)

### Current Sprint / Active Tasks

**UI Overhaul — component summary contract + Svelte web UI + stormsh dashboard**

The web UI moves from format!()-embedded HTML strings in `web.rs` to a Svelte 5
SPA embedded in the binary, and both UIs (web + stormsh TUI) render from one
server-side component-summary contract so they can never drift apart.

- [x] Phase 1: Component summary API — `components.rs` with a uniform
      `{id, kind, label, health, detail, metrics, actions}` summary for every
      component (system, each process, logs, cron, updater, storage, plugins);
      `GET /api/v1/components` + `/ws/components` push
- [x] Phase 2: Svelte 5 + Vite SPA in `web/` — design tokens, card/tile
      component library, dashboard rendered generically from the summary feed;
      built `web/dist` committed and embedded in the binary (rust-embed),
      old `web.rs` string pages removed
- [x] Phase 3: Logs + Terminal views in the SPA (WS live tail, ANSI rendering)
- [x] Phase 4: stormsh Dashboard view rendering the same `/api/v1/components`
      feed as TUI tiles — the console "sum" of each component
- [x] Phase 5: Plugin summaries — optional `[process.ui] summary` URL merged
      into the plugin's component card (best-effort, short timeout)

All five phases shipped in v0.4.0 (2026-08-26), verified end-to-end on dev
(build, tests, live smoke of /api/v1/components, /ui/, legacy redirects, and
the plugin summary merge).

**Follow-on (v0.5.0, same day):**
- [x] Six themes as token-override blocks (Storm/Midnight/Nord/Solar/
      Phosphor/Light), nav picker, localStorage persistence; ANSI palette
      and chart re-color per theme
- [x] Login system — `[api] password`/`auth_token`, session cookies +
      bearer middleware, UI login screen, stormsh `--token`/`STORMD_TOKEN`
- [x] Typed relations (`has_one`/`has_many`/`belongs_to`) in the contract;
      dashboard grid view: nested DataGrid, multi-select bulk actions,
      RelationPicker ("select from a relationship")
- [x] Contract extracted to the `stormview` crate in its OWN repo
      (github.com/glennswest/stormview, private, v0.1.0) — stormd and
      stormsh consume it via git dependency; stormdrive/stormconsole will too

### In Progress

**Docs refresh, second pass (2026-09-27) ✅ done.** No code since cc11049; defaults,
routes, applets (63) and validation re-checked against the source and still
match. What the docs lacked is behaviour filed since as issues:
- [x] README: `mcast.group = "off"` still sends (#27); no limiter on the group
      (#12); pinned stormcast panics on a multibyte char at byte 8192 (#28);
      a failed process's stderr never reaches stormd's own output (#29); an
      open failure logs an ERROR per line (#1); unexpanded `${NODE_IP}` (#3);
      CloudID on a node reaches stormimds (#30, stormimds#9); the log volume
      is the component's `-logs` golden clone; outside a golden, `log_dir`
      belongs on a PVC (on stormcos, the built-in stormblock PVC driver)
- [x] example.toml comments (`off`, `log_dir`); presentation status/planned
      slides; changelog
- Verified: `sc-build 'cargo test -p stormd config::'` on 701e577 passes
      (example.toml parse test).
- Third pass (later 2026-09-27): no code change since cc11049; routes,
      defaults, ports, applets, PVC wording re-checked — docs still match.
- Fourth pass: still no code change; README + presentation now record
      #31 (applets always exit 0) and #32 (API plaintext, anonymous by default).

**Session state (2026-09-27, before the session restart):** nothing in
progress. Open-issue validation done three times today (no code change between; third pass re-checked each against c8efd03, #11 comment points its orphan half at #23):
#1, #3, #7–#12 still real, priorities unchanged (P1 #9 #11; P2 #1 #3 #8 #12;
P3 #7 #10); #4–#6 and #15 closed. Comment mining done twice (second pass: 29 issues
updated since 09-18, every finding already filed): nothing unfiled, one
live sighting added to stormpump#38. Third pass (26 issues updated since 09-25, 19 comments): all
already filed except #32's callers — stormconsole#49 filed (P2, stormd feed TLS/auth),
stormcos#64 told its stormd scrape jobs need TLS + bearer once #32 lands. Fourth pass
(2026-09-28, 19 issues updated since 09-27, 6 comments): #10's newest comment
(the proxy drops `Authorization`, which breaks stormstorage UI writes) split out as
#34 (P2); stormcos#64 told that stormd exports no per-process RSS/CPU/fds (#33). Newer open issues #23–#30 are not yet
validated or started. Next: pick up by priority.
Fifth mining pass (2026-10-09, 37 issues updated since 09-29, 45 comments): filed
#53 (proxy: WebSocket/Location/2 MB, P3) and #54 (API health in-flight capture, P3);
#41/#42 noted as stale (same ad-hoc #34 run as #39/#40); everything else already filed.
Sixth mining pass (2026-10-09, 40 issues updated since 10-06, 47 comments): filed
rustkube-node#218 (kubelet can't read a TLS/auth stormd, P2) and #55 (group-signal
exit unlogged at shutdown, P3); everything else already filed.

**Docs refresh, third round (2026-10-09) ✅ done.** README, presentation and
plugin-ui.md re-derived from the code for everything since 2026-10-02 (v0.8.0
+ #48 #49 #44 #31 #10 #29 #7 on main): versions, shutdown deadline, module
map, full validation list, `process_ready` emitted, #29 paragraph, metrics
states, retired `[process.liveness]` on service goldens; presentation status
and planned slides from today's open issues. Filed #56 (P3): shell `ps`/
`liveness`/`status` and the component card read `has_liveness` from the
retired `[process.liveness]`, not `liveness_probe`.

**Issue #32 — API over TLS, no anonymous access (2026-10-06) ✅ done.**
Decisions (from the issue and code, not asked): auth stays "on when any
credential is configured" (stormcos wires the flags per container, and an
unconfigured stormd keeps working, with a warning at start); `/metrics` sits
behind the same auth on the same listener (bearer or client cert — ironprom
supports both, stormcos#64); only `/api/v1/health` + new `/healthz` stay
anonymous for data (the SPA shell and `/api/v1/auth/*` stay open so the login
screen loads).
- [x] `[api] tls_cert_file`/`tls_key_file` (rustls/ring, HTTP/1.1, pair re-read
      on change), `client_ca_file`, `token_file` (re-read on change) — cae99f4
- [x] `/metrics` behind auth, `/healthz`; `--healthcheck` falls back to https
- [x] stormsh `--ca-file`, `--cert`/`--key`, `--token-file` — 1ee62df
- [x] unit tests (TLS, client cert, stranger CA, rotation, token file), medium
      suite (`auth-token` checks /metrics 401/200, /healthz); docs, changelog
- [x] Live on dev via sc-build (real binary, curl): plain http to the port
      fails; healthz/health 200 anon; processes, metrics, stop 401 anon / wrong
      token; 200 with token or client cert; stranger-CA cert and untrusted
      server refused at handshake; token file rotated → old 401, new 200;
      cert files rotated → next handshake serves the new serial; /ws/logs 101;
      mismatched pair → exit 1, nothing spawned; --healthcheck 0 on TLS port
- [x] Told stormcos#81 (config to wire), stormcos#64 (scrape), stormconsole#49
- Test certs are throwaway fixtures in `crates/stormd/src/tls_fixtures.rs`
  (100-year test CA; the `.pem` gitignore is why they are Rust constants)

**Issue #52 — `[api_health] state_file` for PID 1 (2026-10-09) ✅ done.** rustc
warnings cleared in the same change; clippy + deny-warnings is #58.
To stormpump#127's reader (c0c9e24): after every probe (and once at start,
empty) write `{"updated", "items":[ApiHealth + interval_secs]}` to
`<file>.tmp`, rename into place; left in place on stop. Also: zero build
warnings (new cross-project rule) — the ones in stormd/stormsh fixed here.

**Issue #7 — dead config keys; unknown keys warned (2026-10-09) ✅ done.**
As the issue proposes (no owner decision needed):
- unknown keys: one WARN per key at load (serde_ignored), never rejected;
- removed (so they now warn as unknown): `[log]`, `[general] pid_file`,
  `[debug] dynamic_log_level`, `[updater] registry`;
- implemented: `[ssh] authorized_keys` (OpenSSH file, re-read on each auth,
  alongside CloudID), `capture_stdout`/`capture_stderr = false` → /dev/null;
- `[stormlog.file] log_dir` set to something other than `[general] log_dir`
  → WARN (it is overridden);
- `process_ready` event emitted when a process becomes ready.
- [x] code + tests (`unknown_keys_are_reported_not_refused`,
      `authorized_keys_tests`); README, changelog; Cargo.lock (serde_ignored)
      from a build job; --locked sc-build, live warnings, medium 19; golden

**Issue #29 — echo a failed run's last lines on stormd's stderr (2026-10-08) ✅ done.** stormlog keeps the last 20 lines per run (`tail`, reset at
spawn_capture); `echo_tail` writes `name| line` to stderr after archive_run,
before "process exited with error" (both exit paths). Test
`stormlog::tail_tests`; live: a failing child's error in stormd's output.

**Issue #25 — dependents woken on a state change (2026-10-08) ✅ done.**
`tokio::sync::Notify` signalled on spawn, exit handling and every ready
change; the wait registers before it checks, 1 s backstop poll. Test
`dependency_wake_tests` (6 chained one-shots < 600 ms; was ≥ 1.25 s).

**Issue #10 — cron timeout kills, jobs side by side; liveness counter
(2026-10-08) ✅ done.** Point 3 (proxy) was #34. `kill_on_drop` + a task per
job (a job still running skips its next fire time); `liveness_failures_total`
monotonic, consecutive count as a gauge. Tests `cron::run_tests`.

**Issue #30 — CloudID refresh on a node reaches stormimds (2026-10-08): part
done, rest waits on stormimds#9 (needs-owner).** Done now (the issue's
"either way"): a 404 names the likely cause (`cloudid::refusal`, test). The
real fix, refuse `[ssh] owner` on a node (master's recommendation) or point
`cloudid_url` at cloudid, depends on stormimds#9; proposed after it.

**Issue #28 — stormcast 0.1.0 → 0.1.1 (2026-10-08) ✅ done.** Lock
edited by hand (`3cec734`, no deps): the API stormd uses (send_at,
strip_ansi, DEFAULT_GROUP, Limiter/Verdict/RATE_PER_SEC/BURST, offer, flush)
is unchanged at main. Regression test `mcast::wire_tests`. stormcast#5's
header repair changes nothing for stormd's process names unless they hold a
space or non-ASCII. Verify with --locked build + tests; golden.

**Issue #26 — a group signal's child exit logged as a crash (2026-10-08) ✅ done.** As proposed: the signal path sets the shutdown flag first thing
(`Supervisor::begin_shutdown`); an exit by SIGTERM/SIGINT/SIGHUP waits up
to 300 ms for shutdown to begin before it is counted as a crash. Test
`group_signal_tests`. Live: `timeout -s TERM 3 stormd` with a sleeper.

**Issue #48 (P0, owner URGENT) — Kubernetes-style probes, backoff, events
(2026-10-08) ✅ done (stormd side).** Owner's spec: latest comments on #48. Steps, each
pushed + built + golden:
- [x] 1. startup/liveness/readiness probes (`probes.rs`, http_get/tcp_socket/
      exec, k8s fields/defaults/camelCase aliases), startup gates the rest,
      liveness → SIGTERM/SIGKILL → restart policy, readiness → ready only;
      old `[process.liveness]` retired (warn, never kills). Tests
      `supervisor::liveness_tests` (rewritten), `probes::tests`
- [x] 2. restart policy Always/OnFailure/Never, backoff 10 s ×2 to 5 min,
      reset after 10 min, state CrashLoopBackOff (`backoff_tests`); opt-in
      per process, old fields untouched when unset
- [x] 3. Kubernetes events (Unhealthy/Killing/BackOff/Started/Created):
      `k8sevents.rs`, GET /api/v1/events?since=; rustkube-node#215 told the
      interface (and that node_health.rs reads the retired [process.liveness])
- [x] 4. grpc probes (reqwest http2); Cargo.lock diff taken from a build job
      and committed
- [x] verified: --locked build, 93 unit tests, medium 19; live Dell-style run
      (8 s start under startupProbe never killed, ready after, old liveness
      ignored, events Created/Started/Unhealthy x9/BackOff, CrashLoopBackOff);
      found + fixed: an exited/stopped process stayed `ready`
- [x] golden; told stormcos (#186) how to move fastetcd / the apiserver to
      startup probes; mirror-pod publishing is rustkube-node#215

**Issue #44 — restart that waits for health (+ #46) (2026-10-08) ✅ done.**
Decisions (from the issue and code, not asked; #48 removes liveness *kills*,
not the probe result used here):
- "Healthy for a run" = every check the process has, passed after that
  spawn: ready_probe passed (needs #46: readiness is now watched per run,
  from spawn_process, so a restart is ready again), liveness probe passed
  for that run, every `[[process.api]]` healthy with a probe after the
  spawn. None of these → healthy once Running 3 s (SETTLE).
- `POST …/restart?wait=healthy&timeout=N` (default 60, max 3600): 200
  `{status: healthy, run, waited_ms}`; 504 `{status: timeout, run, waiting_on}`
  with the process left running; 502 `{status: exited, exit_code}` if the run
  ended. Plain restart unchanged.
- Status gains `run`, `liveness_passed_at` + `liveness_passed_run`,
  `ready_at`, `healthy`.
- [x] supervisor: per-run readiness (#46), liveness pass record, health_of(),
      restart_and_wait(); API; unit (`wait_healthy_tests`) + medium
      (`restart-wait-healthy`) tests; README, changelog
- [x] sc-build: 79 unit tests incl. `wait_healthy_tests`; medium 19 incl.
      `restart-wait-healthy` (200 after 2062 ms, 504 naming the ready_probe);
      golden; closed #44, #46; told stormcos#25

**Issue #49 (P0) — API health probes (2026-10-08) ✅ done.** As the issue
specifies (stormcos#458). Decisions from the code, not asked:
- `[[process.api]]`: `name`, `url` (GET), `interval_secs` 15, `timeout_secs`
  5, `p50_ms`/`p99_ms` budgets (optional), `initial_delay_secs` 10 (a
  starting process is not stalled), `token_file` (bearer, re-read) or
  `client_cert_file` + `client_key_file`, `restart_after_stalled_secs`
  (unset = never). Certificates not verified (as liveness).
- States: `healthy`; `slow` = this answer over `p99_ms`, or the p50 of the
  last 20 answers over `p50_ms`; `stalled` = no answer within the timeout;
  `down` = refused / error / HTTP status ≥ 400 (a 401 is a misconfigured
  probe, and saying so beats calling it healthy). `unknown` before the first.
- One task per API per run, aborted with the run (the #45 pattern). State is
  kept across runs per (process, api).
- A change logs once: `healthy` INFO, `slow` WARN, `stalled`/`down` ERROR,
  with the latency, the state before and how long it lasted. Appended to
  `/system-data/history/api/<process>.jsonl` when `/system-data/history`
  exists. `GET /api/v1/health/apis` (behind auth) serves the current state,
  since, last latency, p50/p99 seen and the last error.
- `restart_after_stalled_secs = N`: stalled for N s → ERROR, SIGTERM to that
  run's pid (SIGKILL after its stop timeout), so the normal restart policy
  takes the exit. "What the process reports in flight" is not part of any
  protocol yet, so it is left out.
- [x] config + validation; apihealth.rs (classify, store, history); task in
      spawn_process; API route; unit tests; README, example.toml, changelog
- [x] sc-build (P0 slot): 75 stormd tests incl. `apihealth::tests` and
      `supervisor::api_health_tests`; live: fine→healthy 1 ms, stuck→stalled,
      refused→down, one log line each, web server stopped → down; medium 18
- [x] golden recorded; told stormcos#458

**Issue #24 — test image per the updated standard (2026-10-07) ✅ done (verified 2026-10-09).**
Standard (stormcentral docs/test-standard.md, runner `BUILD_PUSH`): one image,
`test/Containerfile` with the repo root as context, no container runtime
(#121), `/test <suite>`, `test/build.sh` runs first with CARGO_TARGET_DIR set.
- [x] build.sh → binaries in `test/out/` (gitignored), no podman; Containerfile
      FROM scratch COPY test/out/…; no SUITE arg; argv[1] suite (helper kept);
      Job `command: ["/test", "${SUITE}"]`; README; changelog (4973039)
- [ ] verify under sc-build: build.sh, then the runner's own Containerfile
      interpreter (from stormcentral testruns.rs) up to the push; run
      `test/out/stormd-test short` — NOT YET RUN: 2026-10-07 build VMs drained
      by the master for the 11.95 install test on pvetest1. Script:
      build.sh with CARGO_TARGET_DIR, then the python from stormcentral
      src/testruns.rs (`<<'PY'` block) with dest 127.0.0.1:1, then
      `./test short|medium|bogus` in the image root

**Issue #31 — applets exit non-zero on failure (2026-10-07) ✅ done.**
`ShellOutput.status` (0/1/2), `error`/`usage`/`failed_if` helpers; every
usage return → 2, error returns and loop-collected errors → 1, grep no match
→ 1, ping with no answer → 1; `execute_standalone` returns it. New `test` /
`[` applet (stormcos#81: wait for a minted cert). Error text stays on stdout
(unchanged).
- [x] code, unit tests (`file::status_tests`), README, presentation, changelog
- [x] sc-build: 68 unit tests, live symlink statuses, a stormd one-shot
      `test -e` holding its dependent; closed; told stormcos#81
- Found on the way and fixed: every applet read all of stdin first, so
  under stormd (child stdin never closes) `test`/`cat FILE` hung

**Issue #8 — updater starts an image process from its existing rootfs
(2026-10-07) ✅ done, v0.8.0.** Each successful pull+pivot writes
`<rootfs_dir>/<name>.image.json` (image, digest, cmd, env, working_dir).
At start, a rootfs with that record → register + build the config from it
+ start, current_digest = the record's (so a newer registry digest is
pulled on the first poll — before, the registry's digest was recorded and an
image published while stormd was down never arrived). A rootfs without a
record (older stormd) → pull again. `image` with `[updater] enabled = false`
→ one ERROR per process at start (not a validation refusal: a container that
booted before must still boot).
- [x] record write/read, config builder shared with pivot, start branch;
      ERROR for disabled updater; unit tests (`updater::record_tests`);
      README, changelog
- [x] sc-build at v0.8.0: workspace tests, short + medium suites, live checks pass

**Issue #1 — log file open failures: recreate the dir, back off (2026-10-07) ✅ done, v0.8.0.** As the issue asks: on ENOENT `create_dir_all(log_dir)` and
retry once; a persistent open/write failure logs one ERROR, retries at most
every 1 s (lines in between are not written to the file — the group and
streams still get them), reminds at most once a minute with the count, and
on recovery logs INFO and writes one marker line into the file saying how
many lines it is missing.
- [x] file.rs failure state + write_at(now); unit tests
      (`file::failure_tests`); README, changelog
- [x] sc-build at v0.8.0: workspace tests, short + medium suites, live checks pass

**Issue #23 — bind the API before anything is spawned (2026-10-07) ✅ done, v0.8.0.** The issue's first resolution: the bind moves up next to the TLS
load, before cron, the updater and the start order, so a taken port stops
stormd with nothing running. Medium test: a port already held → exit 1, the
one-shot never ran.
- [x] main.rs; medium `api-port-taken`; README startup order, presentation,
      changelog
- [x] sc-build at v0.8.0: workspace tests, short + medium suites, live checks pass

**Issue #12 — stormcast's limiter on the group (2026-10-07) ✅ done, v0.8.0.**
Pinned stormcast 9244121 already has `Limiter`/`Verdict`/`RATE_PER_SEC`
(200)/`BURST` (2000) — no `cargo update`. One `Limiter` per process name in
the mcast adapter (not `Limiters`: that has no per-source flush at this
rev); notices go out first as Notice lines from the same process, the line
only on `Emit`; flush when the process's output ends. Only the group is
limited — file, terminal and streams keep every line. Decision (not asked):
Emergency entries (stormd's own `*** PROCESS CRASHED ***`) skip the rate
limit, so a flood cannot hide the crash.
- [x] mcast adapter + flush at end of output; unit tests (`mcast::gate_tests`);
      README, changelog
- [x] sc-build at v0.8.0: workspace tests, short + medium suites, live checks pass

**Issue #33 — per-process RSS, CPU, open fds in /metrics (2026-10-07) ✅ done, v0.8.0.** As proposed: for each running process, `/proc/<pid>/status`
(VmRSS, VmSize), `/proc/<pid>/stat` (utime+stime / CLK_TCK), count of
`/proc/<pid>/fd`, as `stormd_process_resident_memory_bytes`,
`stormd_process_virtual_memory_bytes`, `stormd_process_cpu_seconds_total`,
`stormd_process_open_fds` with `{container,process}`. The direct child only
(its own children are not summed) — documented. Unit tests: parsers, and a
spawned child whose fd count moves.
- [x] stats.rs proc_usage + parsers; metrics; unit tests (`proc_usage_tests`),
      medium `metrics` checks the worker's series; README, changelog
- [x] sc-build at v0.8.0: workspace tests, short + medium suites, live checks pass

**Issue #3 — never spawn with an unexpanded ${NODE_IP} (2026-10-07) ✅ done, v0.8.0.**
Decisions (from the issue and code, not asked): only stormd's own names
(`NODE_IP`, `NODE_NAME`) count — `${HOME}` in an `sh -c` script is the
script's. A process whose expanded args or applied env still hold one is not
spawned: one ERROR naming the process, the name and why (no address / no
route; empty hostname). In the start order and before a restart it **waits**
(re-resolving every 1 s, shutdown-aware) instead of failing, as the issue
suggests: a node without an address is blocked, not failed, and DHCP may
still come. An API start is refused with that message. The issue's stormpump
log-volume EINVAL is stormpump's (filed there if not already).
- [x] nodevars::unexpanded + reason; supervisor check in spawn_process and
      wait in start_all + both restart paths; unit tests (`node_vars_tests`,
      `nodevars::tests::unexpanded_names_only_stormds_own`)
- [x] README, changelog; the log-volume EINVAL is stormpump#38 (closed)
- [x] sc-build at v0.8.0: workspace tests, short + medium suites, live checks pass

**Issue #36 — a process names goldens; stormd attaches them read-only and
presents them (2026-10-07) ✅ shipped, v0.8.0 (real mount path: first stormcos user).** Owner chose minismbd#11 option A.
Facts (stormblock, stormpump code): engine `POST /api/v1/volumes/{id}/attach`
`{mode:"ro", transport:"ublk", holder}` → `{"transport":"ublk","device_hint":
"/dev/ublkbN"}` (local node only; rw of a golden is 409); `DELETE …/attach`
releases (409 while mounted); names resolve via `GET /api/v1/volumes?kind=golden`;
Bearer token at `/run/stormblock/engine/api_token` (bound into a container by a
`mount sbrun /run/stormblock ro` stanza); engine on `:9090`. stormdbase
containers run as root with no capability drop (stormpump), own mount
namespace, a tmpfs `/dev` without the ublk node — so stormd mknods it from
`/sys/block/<dev>/dev`. Design (decisions from the code, not asked):
- `[[process.golden]]`: `name`, `golden` (name) or `volume_id`, `content =
  "filesystem" | "image"`, `path` (default `<[goldens] dir>/<name>`,
  dir default `/goldens`), `fstype` (default ext4), image `owner` uid:gid +
  `mode` (default 0:0, 0444), optional `size_bytes` (reported, not enforced:
  minismbd limits reads by its entry's size). `[goldens] engine_url` (default
  `http://${NODE_IP}:9090`), `token_file`.
- Before a process's first spawn (after wait_for_files): resolve, attach ro
  over ublk, mknod; filesystem → mount MS_RDONLY at path; image → the device
  node at path, chowned/chmodded. Failure retries every 2 s (log once),
  shutdown-aware. On shutdown: unmount, detach.
- `GET /api/v1/goldens`; swap `PUT /api/v1/processes/{p}/goldens/{name}`
  `{golden|volume_id}`: stop the process, unmount, detach, attach the new
  one, present it, start the process. In memory: a stormd restart goes back
  to the config.
- Engine client + host ops (mknod/mount/umount/chown) behind a trait; unit
  tests against an in-process engine stand-in and a recording fake. The
  real mount path needs root + a node engine: not testable in sc-build.
- [x] config + validation; goldens.rs (client, host ops, present/release)
- [x] supervisor hook (first start, shutdown), swap; API routes
- [x] unit tests (goldens::tests against an engine stand-in + recording
      host, config::tests::goldens_are_validated); README § Goldens,
      example.toml, changelog
- [x] capabilities: noted on stormpump#47 (boot.d services keep theirs)
- [x] sc-build at v0.8.0: workspace tests, short + medium suites, live checks pass
- [x] told minismbd#7 the config shape and paths

**Issue #43 — record a stormd input golden at head (2026-10-07) ✅ done.**
Owner: yes, after each issue that passes sc-build (CLAUDE.md "How it ships",
memory updated). The master requested it at 36a95f0 →
`golden-stormd-3e9d395470bf` (verify: 33554432 bytes match), which carries
#37, #45 and everything through #31. Previously: stormcentral now lists stormd as an `input` golden
(golden-stormd-fe3f126b72c8, 8edb89c), which conflicts with the 2026-09-26 rule
"stormd never requests goldens". Asked on #43 (needs-owner) whether stormd
sessions should run `component build stormd`. If yes: only after sc-build
passes on head (head carries unbuilt #9/#11/#38), then update "How it ships"
and the memory.

**Issue #38 — `[process] wait_for_files` (2026-10-07) ✅ done, v0.8.0.** As the
issue specifies: spawn only once every listed file exists, polled every
250 ms, one log line naming what is missing (and one when they appear), no
restart or cool-off counted. Decisions from the code: in the start order,
after `depends_on` and before `startup_delay_secs`, like `depends_on` (a later
process waits behind it); ends when shutdown begins; restarts and API starts
do not wait (the files existed when it first started); paths must be
absolute (validated), `${NODE_IP}`/`${NODE_NAME}` expanded.
- [x] config + validation + supervisor wait; unit tests
      (`wait_for_files_tests`, `config::tests::wait_for_files_must_be_absolute`)
- [x] medium `wait-for-files`; README, example.toml, presentation, changelog
- [x] sc-build at v0.8.0: workspace tests, short + medium suites, live checks pass

**Issue #9 — stop is SIGTERM, then SIGKILL after a grace (2026-10-07) ✅ done, v0.8.0.**
The issue's proposal, no owner decision needed. Decisions from the code:
- `[process] stop_timeout_secs` (default 10; 0 = SIGKILL at once). The run's
  monitor task, on a stop request, sends SIGTERM to the child's pid, waits up
  to the timeout for it to exit, then SIGKILLs; the exit code is recorded.
  No exit event (a requested stop is not a crash, as before).
- `stop_all` stops in reverse dependency order: tiers by `depends_on` depth,
  deepest (dependents) first, each tier signalled together and waited for
  (its max timeout + 2 s) before the next. The shutdown watchdog is no
  longer a fixed 30 s: the sum of tier waits + 20 s, at least 30 s.
- `restart_process` and the updater's pivot wait until the old run is gone
  (timeout + 2 s) instead of a fixed 500 ms / 5 s — a slow exit no longer
  races the new run for its port.
- Liveness keeps SIGUSR1 → 5 s → SIGKILL (and #48 may remove it).
- [x] config key + monitor SIGTERM/grace/SIGKILL + wait_stopped helper
- [x] stop_all tiers; watchdog from config; restart/updater waits
- [x] unit tests (`stop_tests`): TERM handler runs and exit is recorded; TERM
      ignored → SIGKILL after the timeout; stop_all stops a dependent before
      its dependency; timeout 0 → SIGKILL; tiers + cycle
- [x] README, example.toml, design doc, presentation, changelog
- [x] sc-build at v0.8.0: `stop_tests`, `test/live-stop.sh` (stop 2.01 s, restart waits 1.03 s, app before db), suites

**Issue #45 (P0) — a liveness task outlives its run (2026-10-07) ✅ done.**
Cause: the task stops only when it reads `state != Running`, so one asleep in
`initial_delay_secs` (or mid-probe) across a crash + restart wakes on the new
run and probes it at once; `liveness_failures` is never reset at spawn, so
the next run starts at the old count. Fix (master's "stop the stale-task bug
now"; removing liveness kills altogether waits on the owner's yes/no on #45):
- [x] `run` generation on each spawn; reset `liveness_failures` at spawn
- [x] liveness task spawned per run, aborted by the run's monitor task when
      the child exits or is killed; every check and the SIGUSR1/SIGKILL act
      only if the run is still the task's (signal the run's own pid)
- [x] live-task count per process (drop guard) for the test
- [x] test `liveness_tests::a_liveness_task_ends_with_its_run`; README,
      changelog (commits ec…/6b51d5b)
- [x] sc-build on 5fa18be (build VM, SC_BUILD_P0): `cargo build && cargo test
      -p stormd` passes, 40 tests incl. the new one. Running the new test
      against the pre-fix code was not possible: the build VM's checkout has
      no earlier history (the attempt filed #47, closed as not a failure)
- [x] closed #45; the removal question split out as #48 (needs-owner)
- Found on the way: #46 (a restarted process with a ready_probe is never
  ready again)

**Issue #11 — refuse init under an unknown argv[0] (2026-10-06) ✅ done, v0.8.0.**
Decisions (from the code and stormcos, not asked): init only when argv[0]'s
basename is `stormd`, `stormd-*` or `stormd.*` (a renamed copy), or empty;
an applet runs the applet; anything else → `stormd: <name>: not a stormd
applet (see stormd --list-commands)`, exit 127, before logging, config or
any spawn. No standalone `ps`: stormcos links exactly `--list-commands` and
fails a golden with `/bin/ps` (stormcos#66), so adding one would break it.
- [x] pure `classify_argv0` in shell/mod.rs + main.rs dispatch; unit tests
- [x] medium-suite check `unknown-argv0-refused`: argv[0] `ps` → exit 127,
      the one-shot never ran
- [x] README (Running, stormdbase note, medium row), presentation, changelog
- [x] sc-build: live `ps`/`ls` symlinks, medium `unknown-argv0-refused`; closed

**Issue #37 — `[process] env_default` (2026-10-06) ✅ done (c34b849).** For
stormcos#282 / stormpump#88 env.d overrides: `env` overrides the inherited
environment, so env.d can never win. Precedence at spawn: `env` > inherited
(stormd's own environment, i.e. env.d) > `env_default`. A key present but
empty in stormd's environment counts as set. Values expanded like `env`.
- [x] config key + pure helper (testable without touching the process env)
      + spawn; unit tests; README table, example.toml, changelog; sc-build +
      live check (inherited vs not)
- Verified via sc-build on c34b849: build + all tests (3 `env_tests`, example.toml
  parse). Live: not inherited → `FASTETCD_DATA_DIR=/data/fastetcd`,
  `APISERVER_URL=https://<node ip>:6443`; inherited `/data/fastetcd-fresh` →
  passed through; inherited empty `APISERVER_URL=` → stays empty; a key in both
  `env` and `env_default` (and inherited) → `env`'s value.

**Issue #34 — plugin proxy forwards headers (2026-10-06) ✅ done (3e4719f).**
`/ui/proxy/{name}/…` sent only `Content-Type` + a `String` body and returned
only status + `Content-Type`, so stormstorage's UI bearer retry never arrived.
Decision on #32 interaction (taken from the code, not asked): stormd's
middleware already accepts a session cookie when the bearer is not stormd's,
so the browser path needs nothing new. The proxy strips only *stormd's own*
credentials — an `Authorization` equal to stormd's `auth_token` and the
`stormd_session` cookie — and passes everything else (a plugin's bearer
included). Upstream `Set-Cookie: stormd_session=…` is dropped.
- [x] request headers minus hop-by-hop/`Host`/`Content-Length`; body as bytes;
      any method; response headers minus hop-by-hop (`Set-Cookie`, `Location`
      kept); one shared client, redirects not followed
- [x] tests against an in-process upstream; plugin-ui.md, presentation;
      changelog; sc-build (`cargo build && cargo test` passes)
- Verified live via sc-build: stormd with `auth_token` + password, a Python
  stand-in plugin answering writes 401 without `Bearer plugin-token`. No stormd
  credential → 401 from stormd; session cookie only → plugin's 401; session +
  plugin bearer + 1 MB binary POST → 201, plugin saw the bearer, the query and
  all 1,000,000 bytes, no `stormd_session` cookie, `Set-Cookie` came back;
  stormd's bearer through the proxy → let in, not forwarded. Bodies over 2 MB
  are a 413 (axum default, unchanged from before). No version bump: stormd
  never requests goldens; the fix rides the next stormcos release.

**Issue #19 — CloudID key refresh speaks IMDSv2 (2026-09-26) ✅ done, v0.7.4.** stormimds
(default `security.mode = "both"`) answers a bare GET 401 with an empty body;
`fetch_keys` parsed that as an empty index — no keys, no warning. Plan:
- [x] `PUT /latest/api/token` (`X-aws-ec2-metadata-token-ttl-seconds: 21600`),
      token cached until near expiry, `X-aws-ec2-metadata-token` on each GET;
      a refused PUT falls back to no token (IMDSv1 / other services).
      `Metadata-Flavor: StormIMDS` on every request too, so stormimds's
      `header` mode works as well (real EC2 ignores it)
- [x] non-2xx is an error with its status (index: refresh fails, old keys
      kept; one key: skipped); a 401 with a cached token re-fetches it once
- [x] a failure warns once, not every 30 s, until it changes or recovers
- [x] unit tests against an in-process stand-in (token/both/header/v1
      modes, 401, expiry); README § SSH; changelog; sc-build; patch release
- Keys at `keys/` vs `public-keys/` is stormimds#5 (theirs, open)
- Verified: 4 stand-in unit tests; live against stormimds 743e9e7 built in
  the sc-build job (ignored test `cloudid::tests::live_stormimds`): in
  `token`, `both` and `header` modes a bare GET is 401, stormd's request gets
  `instance-id` with a token issued; the key index is 404 (stormimds#5)

**Issue #22 — exit handling serialized behind restart cooloffs (2026-09-26) ✅ done, v0.7.4.**
`run_exit_handler` awaited each `handle_exit`, which sleeps the cooloff.
- [x] One task per exit event; regression test
      `exit_handler_tests::a_cooloff_does_not_hold_up_another_exit`
- [x] Long suite on dev: resident 64 processes settle 1.1 s (was 6.1 s), a
      128-process wave 4.4 s (was 13.5 s); the rest is the 250 ms dependency
      poll behind one-shots

**Issue #15 — short/medium/long test containers (2026-09-26) ✅ done.** Per
stormcentral `docs/test-standard.md`, shaped like stormcast's `test/`.
Decisions (from the code, not asked):
- The suites run **the stormd of the commit under test** as a child of the
  test binary, with generated configs, and drive it through its REST API;
  the supervised processes are the test binary itself in helper mode
  (`/test helper …`: sleep, exit N, serve TCP/HTTP, write/check files,
  print markers). stormd's job is supervising processes in a container, so
  that is what runs; it needs no hardware (`requires: []`) and no cluster
  API (`automountServiceAccountToken: false`).
- The node's own stormds (control-plane ports 9081–9085, `/api/v1/health`
  is public) are probed read-only; none answering is a **skip**.
- Packaging: stormd needs `stormpull` over `ssh://` (private), so no
  in-container cargo build. `test/build.sh` builds static musl binaries on
  the build box (as the root `Containerfile`s expect); `test/Containerfile`
  is `FROM scratch` + the two binaries. `test/stormd-test.yaml` is the Job.
- `test/` is a workspace member (`stormd-test`) pinned by the one
  `Cargo.lock`, but **not a default member**: the golden build is a bare
  `cargo build --release` in this repo, and a compile error in test code broke
  every service golden once (#20). Build/test it with `-p stormd-test` or
  `--workspace`.
Plan:
- [x] Crate skeleton: env, report (JSON lines, /results, exit 0/1/2), helper
      modes, stormd harness (spawn, config, API client, SIGTERM, residue)
- [x] short: boot + API + start order (one-shot → dependent) + ready probe,
      restart on crash, logs through the API, SIGTERM shutdown with no child
      left, node stormd health (skip if none)
- [x] medium: failure paths — ignore-failed one-shot holds dependents,
      no_restart hold/fail, on_failure=fail, max_restarts, API stop/start/
      restart, API shutdown exitCode, auth 401/200, metrics, components,
      bad config exits 1, SIGTERM with a parked start order, cron
- [x] long: waves until STORM_TIMEOUT — N processes sized from the pod's
      own CPU/memory allowance, churn, drain; start latency, shutdown time,
      stormd RSS/fds and leftover children per wave; regression = fail
- [x] Containerfile, build.sh, Job yaml; README "Tests" section; changelog
- [x] Verify on dev via sc-build: build + run each suite natively (long with
      a short STORM_TIMEOUT); podman build if dev has it
- Verified on dev (sc-build): short 6 pass + node-stormd skip, medium 15 pass
  + skip, long 300 s window (waves, no regression, nothing left); image built
  with podman from `test/build.sh` staging and short passed inside it as uid
  65532. Found and filed: #21 cron never ran (fixed), #22 exit handling is
  serialized behind restart cooloffs (open). Running the image on dev with a
  bind-mounted results dir leaves subuid-owned files that sc-build cannot
  delete — use `podman unshare rm -rf` (or no bind mount) if repeating it.

**Issue #17 — SIGTERM did not stop stormd (2026-09-26) ✅ done, stormd v0.7.2.** Not signal
delivery: the handler fires, then shutdown awaits `start_handle`, and
`start_all` was parked forever in `wait_for_dependencies` on a dependency that
can never be satisfied (the #16 live test: `held` behind a failed one-shot).
Later SIGTERMs land on a tokio stream nobody reads. Same hang as PID 1. Plan:
- [x] Supervisor `shutting_down` flag set by `stop_all`: dependency waits and
      `start_all` give up, `spawn_process` refuses, restart paths stand down
- [x] `stop_all` waits (bounded) for kills to land; main calls it again after
      startup ends (closes the spawn-during-stop window), exits explicitly
- [x] Watchdog: a std thread forces exit 30 s after shutdown begins
- [x] Live test with `timeout -k 5 10` (held dependency, running child);
      README, changelog; sc-build; patch release. Live on dev under
      `timeout -k 5 -s TERM|INT 10`: rc 124 (no SIGKILL needed), exit ~0.1 s
      after the signal, `held` never started, no leftover child. An exit
      during shutdown is logged as a stop — but `timeout` signals the whole
      group, and the child's exit can be handled before the flag is set, so
      one crash + "restarting" line may still appear (restart stands down)

**Issue #16 — a one-shot dependency satisfied at spawn (2026-09-25) ✅ done, stormd v0.7.1.**
`wait_for_dependencies` accepts `Running && ready`, and a process with no
`ready_probe` is ready at spawn — so a one-shot (`on_exit = "stop"`, no probe)
let its dependents through while still working (stormcos#60: node-admin ran
before stormcert-sa wrote the key). Plan:
- [x] Pure helper `dependency_satisfied`: one-shot without probe → only
      `Stopped` with exit code 0; one-shot with probe keeps `Running && ready`;
      `Stopped` counts only after a clean exit (a one-shot failed under
      `on_failure = "ignore"`, or stopped by hand, does not satisfy)
- [x] Log once when a dependent is held behind a one-shot that failed, so the
      wait is not silent
- [x] Unit tests, README § Process supervision, presentation, example.toml
      comment, changelog; sc-build passes on e265647; live on dev: one-shot
      `sa` (sleep 3; touch key) → dependent started only after it exited 0 and
      saw the key; dependent of an `ignore`-failed one-shot never started, one
      WARN logged; patch release v0.7.1

**Issue #6 — presentation (2026-09-24) ✅ done.** `docs/presentation.md`, Marp
Markdown, 12 slides, every claim from the #5 README / the code:
- [x] Deck: purpose, place in stormcos (stormcentral relationships graph:
      depended on by stormcos, rustkube, stormcert, stormlb, stormimds,
      stormipmi, stormblock-registry, stormdrive, stormstorage, stormcoredns,
      stormconsole; depends on stormcast, stormview), moving parts (ASCII
      diagram), features today, planned (open issues), interfaces, shipping,
      status
- [x] Render check: marp-cli 4 on dev via sc-build → 12 slides, exit 0
      (needs `</dev/null` — marp reads a non-TTY stdin as input); README link; changelog
- [x] stormcentral#24: graph lacks fastetcd/rustkube-node/cadvisor → stormd
      though their goldens run under it

**Issue #5 — docs rewritten from the code (2026-09-24) ✅ done.** Owner: every
component re-derives its docs from the source. Also closes #4 (ships in a
golden, not written down). Findings from reading the code:
- `config/example.toml` does not parse — `transport = "nats"` (NATS is gone;
  only `none | webhook`). Fix it, and add a test that parses it so it cannot
  drift again.
- Parsed-but-ignored keys: `[general] pid_file`, all of `[log]`,
  `[ssh] authorized_keys`, `[debug] dynamic_log_level`, `[process]
  capture_stdout/capture_stderr`, `[updater] registry`, `[stormlog.file]
  log_dir` (overridden by `[general] log_dir`). Documented as such; issue filed.
- Updater never starts an image-tracked process whose rootfs already exists
  (e.g. after stormd restarts) — issue filed.
- stormd ships as `/stormd` in every stormdbase golden (stormcos
  `build-goldens.sh` `stormdbase_stage`/`golden_stormd`; stormcentral registry
  kind `special`, "not a golden"). Authority: stormcos `docs/goldens.md`.
- [x] README rewritten from the code (config reference with real defaults,
      API/metrics/health, ports, build via sc-build, how it ships)
- [x] Plugin UI guide → `docs/plugin-ui.md` (stale Dracula style guide
      replaced by stormview tokens); shutdown enhancement → `docs/design/`
      marked implemented; `enhancements/` removed
- [x] `config/example.toml` fixed + parse test; stale code comments (MinIO
      archive, auth-on conditions)
- [x] CLAUDE.md build commands (sc-build, not root@dev / Mac), ships-in-golden
- [x] Issues filed for what the code does not do: #7 dead config keys /
      unknown keys silent, #8 updater never starts an image process whose
      rootfs exists, #9 stop/shutdown are SIGKILL (no SIGTERM), #10 cron
      timeout doesn't kill + liveness "counter" resets + proxy drops headers,
      #11 unknown applet name starts init (with stormcos#66, stormcentral#17:
      goldens link `ps`, which is not an applet)
- [x] sc-build passes on d743d96 (17 tests, incl. the example.toml parse test)

**Issue #2 — non-retryable exit codes (2026-08-30) ✅ done, v0.7.0.** stormconsole#3 was a
config-parse failure that stormd restarted `max_restarts` times, failed the
container, and stormpump restarted for hours; `handle_exit` reduces every
exit to `code == 0`. Adding a per-process carve-out:
- [x] `no_restart_exit_codes = [78]` — exits the process declares not worth
      retrying (sysexits EX_CONFIG 78, EX_USAGE 64); default empty
- [x] `on_no_restart = "hold" | "fail"` — `hold` (default) marks the process
      Failed and leaves the container running; `fail` fails the container.
      Neither counts toward `max_restarts`
- [x] One error line `process exited with a non-retryable code — not
      restarting`, crash entry + ProcessCrashed event carry the code
- [x] Tests (decision helper, config parse), README policy table + config
      reference, example.toml, changelog; build/test on dev (11 pass; live: exit-78 script
      started once, marked failed, stormd stayed up under `hold`, exited 1
      under `fail`; exit-1 control still restarted then failed the container)

### Completed

- [x] Process supervisor (restart policies, dependencies, ready/liveness probes)
- [x] REST API (axum) + WebSocket console/log streaming
- [x] Embedded web dashboard (string-built — being replaced this sprint)
- [x] SSH server + SFTP + busybox-style shell applets
- [x] stormlog: file store, VT100 terminals, multicast log wire (stormcast)
- [x] Cron scheduler, events (webhook), backup, image updater
- [x] Plugin UI reverse proxy (`[process.ui]`) + host-based routing
- [x] stormsh TUI client (processes/terminal/logs)
- [x] Prometheus /metrics endpoint with standard names
- [x] stormdbase multi-arch scratch base image (arm64 + amd64 + armv7)

### Release History

| Version | Date | Summary |
|---------|------|---------|
| v0.1.0 | 2026-02-28 | Initial: supervisor, API, logs, SSH |
| v0.2.0 | 2026-02-28 | Events, cron, backup, web terminal |
| v0.3.0 | 2026-03-01 | Web dashboard, MinIO archival, run segmentation |
| v0.4.0 | 2026-08-26 | Component feed + both dashboards, liveness, busybox, CloudID, updater, stormcast wire |
| v0.5.0 | 2026-08-26 | Themes, login system, relations + grid view, stormview crate extraction |
| v0.6.0 | 2026-08-26 | Named users, 12 themes + server default, card→grid links, UI system moved into stormview (npm) |
| v0.7.0 | 2026-08-30 | `no_restart_exit_codes` / `on_no_restart` — a process can say its exit is not worth retrying (#2) |
| v0.7.1 | 2026-09-26 | A one-shot dependency satisfies when it finishes cleanly, not when it spawns (#16) |
| v0.7.2 | 2026-09-26 | SIGTERM/SIGINT always stop stormd: shutdown ends the start order, is bounded at 30 s (#17) |
| v0.7.3 | 2026-09-26 | Cron jobs run (#21); test container short/medium/long (#15); test crate out of default-members (#20) |
| v0.7.4 | 2026-09-26 | CloudID keys over IMDSv2 (#19); one restart cooloff no longer holds up other exits (#22) |
| v0.8.0 | 2026-10-07 | API TLS + no anonymous access (#32); SIGTERM-first stop, dependents first (#9); `env_default` (#37); `wait_for_files` (#38); goldens for a process (#36); per-process metrics (#33); limiter on the group (#12); per-run liveness (#45); unknown argv[0] refused (#11); API bound first (#23); no unexpanded `${NODE_IP}` (#3); log file back-off (#1); updater starts from an existing rootfs (#8); proxy forwards headers (#34) |

---

## Project Context

### Tech Stack
- Language: Rust (edition 2021), workspace of three crates plus `test/`
  (stormd-test, a member but not a default member)
- Framework: axum 0.8 (REST + WS), tokio, russh (SSH/SFTP), ratatui (stormsh TUI)
- Web UI: Svelte 5 + Vite SPA in `web/`, built to `web/dist` (committed) and
  embedded in the stormd binary
- Build: musl static binaries for x86_64, aarch64, armv7 (scratch containers)

### Key Directories
```
crates/stormd/     — the init/supervisor daemon
  src/supervisor.rs  — process lifecycle, restart policies, probes
  src/api.rs         — REST API router + handlers
  src/components.rs  — component summary contract (UI feed)
  src/ws.rs          — WebSocket console/log/component streaming
  src/web.rs         — embedded SPA serving
  src/config.rs      — TOML config types (config/example.toml is parse-tested)
  src/nodevars.rs    — ${NODE_IP} / ${NODE_NAME} expansion at spawn
  src/probes.rs      — Kubernetes-style startup/liveness/readiness probes
  src/k8sevents.rs   — Kubernetes-shaped events (/api/v1/events)
  src/apihealth.rs   — [[process.api]] health probes
  src/goldens.rs     — [[process.golden]] attach/present/swap
  src/tls.rs         — API TLS, client certs, rotation
  src/shell/         — busybox-style applets
crates/stormlog/   — log store, VT100 terminals, stormcast wire
crates/stormsh/    — TUI client (ratatui)
test/              — stormd-test: the test container (short/medium/long), build.sh,
                     Containerfile, stormd-test.yaml (the Job); not a default member
web/               — Svelte SPA source; web/dist is the built output (committed)
config/            — example.toml (every key; parsed by a unit test)
docs/              — plugin-ui.md, design/ (proposals, with status)
vendor/            — vendored russh-sftp
```

### Build & Test Commands
```bash
# Rust — push first, then from the checkout. Builds the pushed commit on
# dev.g8.lo as an unprivileged user in a scratch dir; never build here, never
# as root, and there is no checkout on dev.
sc-build                          # cargo build && cargo test
sc-build 'cargo test -p stormd'   # any command
sc-build 'cargo clippy'
sc-build 'cargo build --workspace && cargo test --workspace'   # + test crate
# Test container suites against a cargo build (see README "Tests"):
#   STORM_SUITE=short target/debug/stormd-test

# Frontend — commit web/dist (embedded by rust-embed)
cd web && npm install && npm run build
```

### How it ships
stormd is an **input golden** (stormcentral registry `kind = "input"`,
`golden-stormd-<id>`): `/stormd` in every stormdbase golden is taken from it
(stormcos `deploy/build-goldens.sh` `stormdbase_stage` / `golden_stormd`).
Authority: stormcos `docs/goldens.md`. Nothing rebuilds it on its own, so
**after each stormd issue whose work passes sc-build, record it**:
`stormcentral component build stormd --url http://stormcentral.g8.lo` (owner,
2026-10-07, #43 — this replaces the 2026-09-26 "stormd never requests
goldens"). The goldens built on it pick it up; stormcos composes as usual.
Main must still always build. Sibling crates (stormcast, stormview,
stormpull) are pinned by `Cargo.lock`; a fix there needs `cargo update -p`.
stormd's API port per container: fastetcd 9081, rustkube 9082–9085, service
goldens port+100.

### Version Locations
```
crates/stormd/Cargo.toml    → version
crates/stormsh/Cargo.toml   → version
crates/stormlog/Cargo.toml  → version
```

### Known Decisions & Context
- Web UI is a static SPA; no SSR, no node at runtime — assets embedded in the
  ~10 MB binary. `web/dist` is committed so cargo-only builds keep working.
- One component-summary contract (`/api/v1/components`) feeds both the web
  dashboard and stormsh's dashboard view; new subsystems appear in both UIs by
  implementing one summary source in Rust, zero frontend changes.
- Plugin UIs remain iframes behind `/ui/proxy/{name}`; their component card is
  derived from their process state plus an optional `summary` URL.
- Log wire (severities, RFC 5424 framing, multicast) lives in the shared
  `stormcast` crate; fleet log collection is mcastsyslog's job, not stormd's.
- stormpull (image pulling for the updater) comes from the stormbase repo.

---

## MicroDNS REST API Reference

MicroDNS instances run on each network's DNS server. Base URL is `http://<dns-ip>:8080/api/v1`.

| Network | DNS IP | Base URL |
|---------|--------|----------|
| gt | 192.168.200.199 | `http://192.168.200.199:8080/api/v1` |
| g10 | 192.168.10.252 | `http://192.168.10.252:8080/api/v1` |
| g11 | 192.168.11.252 | `http://192.168.11.252:8080/api/v1` |
| gw | 192.168.1.252 | `http://192.168.1.252:8080/api/v1` |

### DNS Zones

```bash
# List all zones
curl -s http://192.168.10.252:8080/api/v1/zones | python3 -m json.tool

# Create a zone
curl -s -X POST http://192.168.10.252:8080/api/v1/zones \
  -H 'Content-Type: application/json' \
  -d '{"name": "example.lo"}'

# Delete a zone
curl -s -X DELETE http://192.168.10.252:8080/api/v1/zones/<zone_id>
```

### DNS Records

```bash
# List all records in a zone
curl -s "http://192.168.10.252:8080/api/v1/zones/<zone_id>/records?limit=100"

# Create an A record
curl -s -X POST http://192.168.10.252:8080/api/v1/zones/<zone_id>/records \
  -H 'Content-Type: application/json' \
  -d '{
    "name": "server1",
    "ttl": 300,
    "data": {"type": "A", "data": "192.168.10.10"},
    "enabled": true
  }'

# Create a CNAME record
curl -s -X POST http://192.168.10.252:8080/api/v1/zones/<zone_id>/records \
  -H 'Content-Type: application/json' \
  -d '{
    "name": "www",
    "ttl": 300,
    "data": {"type": "CNAME", "data": "server1.g10.lo"},
    "enabled": true
  }'

# Create a PTR record (reverse DNS)
curl -s -X POST http://192.168.10.252:8080/api/v1/zones/<reverse_zone_id>/records \
  -H 'Content-Type: application/json' \
  -d '{
    "name": "10",
    "ttl": 300,
    "data": {"type": "PTR", "data": "server1.g10.lo"},
    "enabled": true
  }'

# Update a record
curl -s -X PUT http://192.168.10.252:8080/api/v1/zones/<zone_id>/records/<record_id> \
  -H 'Content-Type: application/json' \
  -d '{
    "data": {"type": "A", "data": "192.168.10.99"},
    "ttl": 600
  }'

# Delete a record
curl -s -X DELETE http://192.168.10.252:8080/api/v1/zones/<zone_id>/records/<record_id>
```

**Supported record types**: A, AAAA, CNAME, MX, NS, PTR, SRV, TXT, CAA

**RecordData formats**:
- `{"type":"A","data":"192.168.1.10"}`
- `{"type":"AAAA","data":"2001:db8::1"}`
- `{"type":"CNAME","data":"target.example.com"}`
- `{"type":"MX","data":{"preference":10,"exchange":"mail.example.com"}}`
- `{"type":"NS","data":"ns1.example.com"}`
- `{"type":"PTR","data":"host.example.com"}`
- `{"type":"SRV","data":{"priority":10,"weight":20,"port":5060,"target":"sip.example.com"}}`
- `{"type":"TXT","data":"v=spf1 mx ~all"}`
- `{"type":"CAA","data":{"flags":0,"tag":"issue","value":"ca.example.com"}}`

**Note**: Duplicate records (same name + type + data) are rejected — the existing record is returned instead.

### DHCP

DHCP pools and reservations are database-driven and managed through each
network's microdns REST API (TOML is first-boot bootstrap only; mkube is
retired). See the DHCP section of the cross-project `~/src/CLAUDE.md` for the
endpoints.

### Other Useful Endpoints

```bash
# Health check
curl -s http://192.168.10.252:8080/api/v1/health

# View logs (with filters)
curl -s "http://192.168.10.252:8080/api/v1/logs?limit=50&level=info&module=dhcp"

# IPAM pools
curl -s http://192.168.10.252:8080/api/v1/ipam/pools

# IPAM allocations
curl -s http://192.168.10.252:8080/api/v1/ipam/allocations

# Zone transfer (import from another DNS server)
curl -s -X POST http://192.168.10.252:8080/api/v1/zones/transfer \
  -H 'Content-Type: application/json' \
  -d '{"zone": "g10.lo", "primary": "192.168.1.51:53"}'
```

---

## Reminders

- Never leave work uncommitted — assume the power could go out right now
- Never skip the changelog
- Never let docs drift from code
- Never commit secrets, keys, tokens, or credentials
- Always update `.gitignore` when introducing new ignorable file types
- Update this work plan before, during, and after tasks
- Commit the work plan itself — it IS the recovery mechanism
- Bump versions according to semver — all version locations must match
- Tag every release — `git tag vX.Y.Z` then push the tag
- When in doubt, commit what you have, document what you did, and push
