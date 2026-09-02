# FORK.md

## What this fork is

`rpmalouin/Hatchdoor` is a fork of [`BatterWorks/Hatchdoor`](https://github.com/BatterWorks/Hatchdoor).
It tracks upstream `main` and carries a small, documented set of deltas on top:
a dependency security-hardening commit, native WebDAV vault support (which this
deployment uses to serve a **Google Drive vault**), and fuse-vault resilience.
Each delta is listed below; everything else tracks upstream unchanged.

## Delta over upstream

Fork point: upstream merge `e631857`. All commits after it, in order:

| Commit | Change |
| ------ | ------ |
| `04fee5e` | `chore(deps)`: patch Rust and frontend dependency security advisories |
| `3cbfbab` | `docs`: document this fork and the dependency security delta |
| `458154b` | `chore`: ignore `MEMORY.md` so repo-local notes never get committed |
| `02dd552` | `feat(webdav)`: add a WebDAV `VaultSource` backend, client, and sync engine |
| `5443413` | `feat(webdav)`: wire WebDAV into the work scheduler, server dispatch, and settings UI |
| `abbd826` | `docs`: record all fork deltas and canonicalize upstream naming |
| `a8bd537` | `chore(docker)`: exclude local agent scaffolding from the build context |
| `6bd0469` | `fix(docker)`: bind sample compose to loopback by default |
| `cecadd1` | `fix(vault)`: write + index resilience on fuse vault mounts (see below) |
| `fd09ce8` | `docs`: add HERMES.md — MCP server build-out playbook |
| `2f67c6b` | `docs(agents)`: document code-review-graph MCP workflow |
| `68a22d0` | `docs`: add Hermes integration blurb to README, link HERMES.md |
| `f13441c` | `fix(webdav)`: WebDAV sync-turn scheduler — the missing Phase D trigger (see below) |
| `f3dd537` | `fix(webdav)`: publish local content after successful sync so activation flips live |
| `faaa77e` | `fix(webdav)`: reconcile remote deletions and edits in the sync engine (see below) |

### Details of the security commit (`04fee5e`)

Bumps dependency lockfiles (no source changes) to clear advisory findings:

- **`Cargo.lock`**
  - `h2` 0.4.15 → 0.4.19 — RUSTSEC-2026-0258 (unbounded empty DATA frames, HTTP/2 DoS on the server transport).
  - `memmap2` 0.9.9 → 0.9.11 — RUSTSEC-2026-0186 (unsound), transitive via candle-core.
- **`frontend/package-lock.json`** (`npm audit fix`, 0 vulnerabilities)
  - `react-router` 7.18.1 → 7.18.3 (GHSA-qwww-vcr4-c8h2, RSC-mode CSRF)
  - `pdfjs-dist`, `dompurify`, `nanoid`, `postcss`, `mermaid`, `fast-uri`, `undici`

Verification: `cargo audit` and `npm audit` both report 0 vulnerabilities; the
frontend typechecks and builds clean. The Rust backend `cargo check` passes; a
full binary link on a host with glibc < 2.38 is blocked by the pre-existing
`ort`/`__isoc23` symbol issue (unrelated to this change) — builds fine in the
project's distroless/glibc ≥ 2.38 Docker image.

### Details of the WebDAV commits (`02dd552`, `5443413`)

The WebDAV additions (see `docs/architecture/work-packet-webdav-vaultsource.md`):
`src/vault/remote/mod.rs` (WebDAV client) and `src/vault/remote/sync.rs` (sync
engine), the `VaultSource::WebDav` variant and mirror checkout, the
`VaultWorkKind::WebDav` + `dispatch_webdav_turn` wiring into `VaultWorkCoordinator`
and the server, and the settings UI (a "WebDAV endpoint" Add-a-Vault option and
an edit-flow mapping). WebDAV is treated deliberately as NOT Git (no git turn)
and reads are served from the local mirror, never the disposable cache, in line
with `ADR-01`.

### Details of the WebDAV scheduler commit (`f13441c`)

The original WebDAV feature shipped the sync engine and the dispatch arm but no
**trigger**: nothing ever queued `VaultWorkKind::WebDav`, so a WebDAV vault
whose mirror did not exist yet sat in `activation: unavailable` forever — the
sync turn that creates the mirror was never requested, and `poll_interval_secs`
was stored but never consumed. This commit adds the missing Phase D:

- **`src/vault/remote/webdav_scheduler.rs`** (new): a `WebDavScheduler` mirroring
  `ManagedGitScheduler` — per-vault entries armed *due immediately* on
  activation (first turn creates the mirror), a 15s tick loop firing
  `VaultWorkKind::WebDav` requests when due (skipping vaults with an admitted
  WebDAV turn), and `record_outcome` re-arming to `poll_interval_secs` on
  success or bounded backoff (30s→60s) on failure.
- **`src/server.rs`**: the scheduler is created beside `managed_git`, cloned
  into the dispatch context (the WebDAV dispatch arm now reports turn outcome),
  threaded through `reconcile_and_reconstruct`, and its tick task is spawned
  and aborted with the git scheduler's.
- **`src/vault_runtime.rs`**: activation calls `webdav.activate(vault_id, poll_interval)`
  for WebDAV-sourced vaults and `deactivate` when a vault leaves that state
  (disabled/disconnected/retired/source changed).
- **`src/app_state.rs`**: `AppState.webdav` field (test and handler wiring).
- **`src/vault_registry.rs`**: `VaultSource::webdav_poll_interval()` accessor.

Follow-up `f3dd537`: after a successful WebDAV sync, the dispatch re-publishes
local-content availability (`publish_local_content_after_sync`, the same
stat + `set_local_content_status` the managed-Git success path uses), so the
live snapshot flips to `activation: Active` (browse + mutate) the moment the
mirror exists — no reconcile or restart required.

Verified: `cargo check` and `cargo check --tests` clean; module-map gate passes;
the full suite (843 tests) passes in the Docker test image (`Dockerfile.test`,
non-root user); deployed live — a WebDAV-sourced vault self-heals from
missing-mirror to indexed-and-browsable without any manual API call.

### Details of the sync reconciliation commit (`faaa77e`)

The sync engine as originally shipped was **additive-only**: it pulled remote
files missing locally and pushed any local file the remote did not list, but it
never refreshed files present on both sides and never removed mirror files
whose remote counterpart had been deleted. On the gdrive build this meant
Obsidian-side deletions stayed in the mirror/index forever *and* were
re-uploaded to gdrive every sync turn (~6.5 min; the mirror copy is treated as
"new local content"), while Obsidian edits to existing notes never reached the
mirror — Hatchdoor silently served stale content. Fix (`src/vault/remote/sync.rs`,
rewritten as a per-directory reconciliation):

- **Pull / refresh:** files missing locally are downloaded; files whose remote
  fingerprint (size + etag from the PROPFIND listing) changed since the last
  successful turn are re-downloaded and overwrite the mirror copy. The
  fingerprints persist in `<mirror>/.hatchdoor/webdav-sync.json` (written
  atomically at the end of a successful turn; absent/corrupt ⇒ first-run
  state, which refreshes everything once and heals pre-existing staleness).
- **Deletion propagation:** a mirror file/dir no longer listed on the remote is
  a stale remnant of a remote deletion and is removed locally — never
  re-uploaded. Emptied local dirs are pruned.
- **Restricted push:** only local files Hatchdoor created/modified since the
  last successful sync (`mtime > last_sync_at`) are PUT'd, after MKCOL'ing any
  missing remote ancestor collections. `.hatchdoor*` names are never synced.
- **404 tolerance:** a subcollection that 404s mid-walk is treated as empty so
  a deletion racing the turn can no longer abort the whole sync; a 404 on the
  ROOT collection stays turn-fatal so a misconfigured source cannot classify
  the whole mirror as stale and wipe it.
- `src/vault/exclude.rs`: `.hatchdoor/` added to the built-in noise patterns so
  the sidecar never wakes the vault watcher or indexer.
- `src/vault_runtime.rs`: turn outcome (`pulled/refreshed/pushed/deleted/
  created_dirs/errors`) logged at info after each successful sync.

Verified: 9 new unit tests in `sync.rs`; 852 total tests pass in the Docker
test image; live-verified on the gdrive build — a folder deleted in Obsidian
stays deleted on gdrive across sync turns (no resurrection), the mirror drops
the stale tree on the next turn, and edited notes refresh in the mirror.

### Details of the fuse resilience commit (`cecadd1`)

Two fixes required by the Google Drive deployment (see below), which sits on a
fuse-style mount where the stock Hatchdoor write/index paths fail:

- `src/vault/write/fs_ops.rs`: `rename_exchange` falls back to a three-rename
  emulation when `renameat2(RENAME_EXCHANGE)` is unsupported (EINVAL/ENOSYS/
  EOPNOTSUPP — fuse/rclone, NFS). Without it every hash-guarded write/move
  fails with `os error 22`.
- `src/cache/populate.rs`: delete a row that owns a note's slug at a different
  `relative_path` before the upsert, so parallel-tree slug families (5-6x
  `_Inbox`/`_Areas`/`Home` basenames) no longer abort the index build with
  `UNIQUE constraint failed: notes.slug`.

## The gdrive build: how this fork serves a Google Drive vault

This fork exists because the deployment vault lives on **Google Drive**, and
Hatchdoor (stock or Docker Hub) cannot read it directly. Two pieces make it work:
the container stack and the MCP surface.

### Container stack (what runs)

`/appdata/A--docker_stacks/Hatchdoor/docker-compose.yml` runs two containers:

1. **`hatchdoor`** (image `hatchdoor:local`, built from THIS fork — the
   `build:` context is `/appdata/Hatchdoor`, so the WebDAV + fuse-resilience
   deltas are baked in). HTTP on `:42824`, MCP on `/mcp`.
2. **`rclone-webdav`** sidecar (`rclone/rclone:latest`): `rclone serve webdav
   gdrive:MyObsidian` on `:42825`, credentials `WEBDAV_USER`/`WEBDAV_PASS` from
   the stack `.env` (rclone config lives in `./rclone`). This is the Google
   Drive front door: rclone talks to the Drive API over HTTPS; no fuse, no
   `/mnt/gdrive` mount involved.

The vault is then registered in Hatchdoor as a **WebDAV source**
(`list_vaults`: `source: { type: web_dav, url: http://rclone-webdav:42825,
poll_interval_secs: 300 }`, vault id `0851e3e7-2daf-4e73-aff2-f074f282c5c6`).
Hatchdoor's sync engine pulls the remote collection into a **local mirror** at
`<STATE>/vaults/<id>/webdav` (container `/data/state/vaults/<id>/webdav`), and
all reads/writes serve from that mirror — the Google Drive remote is only
touched through the WebDAV sidecar. The scheduler commits (`f13441c`,
`f3dd537`) are what make this flow automatic: activation arms a sync turn, the
mirror is created, and the vault flips to `active` (browse + mutate + search)
without any manual API call.

### MCP (what agents see)

Hatchdoor's embedded MCP server at `http://127.0.0.1:42824/mcp` exposes the
vault to agents — 35 tools, prefixed `mcp_hatchdoor_*` in Hermes (read/search/
create/edit/move/delete/archive notes, graph, tree, stats, vault admin,
attachments). It is registered in Hermes with its own bearer token
(`HATCHDOOR_MCP_BEARER_TOKEN`, synced into `~/.hermes/.env`, never committed),
and agents are *forced* through it: `approvals.deny` blocks any terminal
command containing `/mnt/gdrive` or `rclone ... gdrive`, and the
`vault-maintenance`/`vaultagent` profiles have the `file` and `code_execution`
toolsets disabled, so the vault is only ever touched via `mcp_hatchdoor_*`.
The complete playbook — registration, token sync, the real 35-tool surface,
operation templates, the hash-guard rule, the VaultAgent role, drift-detection
cron — is in [`HERMES.md`](HERMES.md).

## Syncing with upstream

```bash
git fetch upstream          # assumes upstream = https://github.com/BatterWorks/Hatchdoor
git rebase upstream/main    # keep the fork's deltas on top of upstream
git push --force-with-lease origin main
```

If upstream merges an equivalent security fix, that delta can simply be dropped:
`git reset --hard upstream/main` and archive this fork. (The WebDAV feature
delta is expected to track upstream as the project evolves, or to be upstreamed
itself.)

## Deliberately NOT included

No secrets, credentials, or machine-local configuration are committed. The
WebDAV feature adds no credentials beyond the existing `https_credentials`
mechanism, and those are stored only in Hatchdoor's own backend secrets store,
never committed. The rclone config, `WEBDAV_USER`/`WEBDAV_PASS`, and all
`HATCHDOOR_*` tokens live in the *stack's* `.env` (`/appdata/A--docker_stacks/
Hatchdoor/.env`), which is outside this repo and never pushed. Local, untracked
agent scaffolding (`.codebuddy/`, `.gemini/`, `.mcp.json`, etc.) is kept out of
version control. `MEMORY.md` is gitignored (`458154b`) — the committed docs for
agents are `FORK.md`, `HERMES.md`, and `SPEC.md`.
