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

### Current Version: stormd `v0.5.0` · stormsh `v0.4.0` · stormlog `v0.3.0` · stormview `v0.1.0` (own repo)

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

(nothing)

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

---

## Project Context

### Tech Stack
- Language: Rust (edition 2021), workspace of three crates
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
  src/config.rs      — TOML config types
  src/shell/         — busybox-style applets
crates/stormlog/   — log store, VT100 terminals, stormcast wire
crates/stormsh/    — TUI client (ratatui)
web/               — Svelte SPA source; web/dist is the built output (committed)
config/            — example/deploy configs
vendor/            — vendored russh-sftp
```

### Build & Test Commands
```bash
# Frontend (on the Mac; commit web/dist)
cd web && npm install && npm run build

# Rust — ALWAYS on root@dev.g8.lo (see ~/CLAUDE.md), never on the Mac
cargo build --release
cargo test
cargo clippy
```

### Version Locations
```
crates/stormd/Cargo.toml    → version
crates/stormsh/Cargo.toml   → version
crates/stormlog/Cargo.toml  → version
```

### Known Decisions & Context
- Web UI is a static SPA; no SSR, no node at runtime — assets embedded in the
  9 MB binary. `web/dist` is committed so cargo-only builds keep working.
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

DHCP pools and reservations are configured via TOML config files, not REST API. Config files live in the microdns ConfigMap mounted into each DNS pod.

```bash
# Check DHCP status
curl -s http://192.168.10.252:8080/api/v1/dhcp/status

# List active leases
curl -s http://192.168.10.252:8080/api/v1/leases
```

**To add/modify DHCP reservations**: Edit the microdns config in mkube's ConfigMap. The config is generated from `config/deploy/microdns-<network>.toml` in the microdns repo.

DHCP reservation format in TOML config:
```toml
[[dhcp.v4.reservations]]
mac = "AC:1F:6B:8A:A7:9C"
ip = "192.168.10.10"
hostname = "server1"
```

DHCP pool format:
```toml
[[dhcp.v4.pools]]
range_start = "192.168.10.10"
range_end = "192.168.10.210"
subnet = "192.168.10.0/24"
gateway = "192.168.10.1"
dns = ["192.168.1.252"]
domain = "g10.lo"
lease_time_secs = 600
next_server = "192.168.10.200"   # PXE TFTP server
boot_file = "undionly.kpxe"       # PXE boot file
```

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
