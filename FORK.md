# FORK.md

## What this fork is

`rpmalouin/Hatchdoor` is a fork of [`BatterWorks/Hatchdoor`](https://github.com/BatterWorks/Hatchdoor).
It tracks upstream `main` and carries a small, documented set of deltas on top:
a dependency security-hardening commit and fuse-vault write/index resilience.
An earlier delta — a native **WebDAV `VaultSource`** with a reconciling sync engine,
built so this deployment could attach a Google Drive vault through an `rclone serve
webdav` sidecar — has been **removed** (see *Removal of the WebDAV vault source*
below). The deployment it served now mounts its vault over SMB and registers it as a
plain `Local` source, which needs no fork code. Each remaining delta is listed below;
everything else tracks upstream unchanged.

## Delta over upstream

Fork base: upstream **v2.6.1** (upstream merge `c2b9861`, 2026-09-08), pulled into the
fork by the merge commit `4e568cc` (2026-09-14) — 176 upstream commits on top of the
previous fork point `e631857` (v2.5.0). Every delta below was carried through that
merge; the table lists the fork's own commits since `e631857`, in order:

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
| `c06ec4a` | `docs(deploy)`: document the gdrive build — WebDAV vault source, MCP wiring, sample compose + `.env` |
| `620bc36` | `docs`: record the WebDAV sync reconciliation delta in FORK.md and CHANGELOG |
| `ae5e7be` | `docs`: note the reconciling WebDAV sync engine in the README fork banner |
| `2e9ba83` | `docs`: record gdrive-build operational caveats in FORK.md |
| `a08bde2` | `docs(hermes)`: write the raw MCP probe's token variable as `<VAR>` (no `$`), which the context-file scanner otherwise blocks |
| `f5c5da4` | `docs(hermes)`: document cron alerting, correct the drift-job spec, list unlisted deltas |
| `4dae54a` | `docs(hermes)`: document how OpsBrain consumes the drift report, incl. the Z-timestamp trap |
| `4e568cc` | `chore(merge)`: merge upstream v2.6.1 into the fork (176 commits); 12 conflicted files resolved, all WebDAV deltas preserved (see below) |
| `3594683` | `docs`: record the upstream v2.6.1 merge (fork base, deltas, gates, sync procedure) |
| `d6d1f05` | `fix(webdav)`: publish local edits to notes the remote still lists (see below) |
| `85b4f19` | `fix(webdav)`: never heal a copy written in the same second as the last sync |
| `e204450` | `docs`: record the WebDAV publish/heal deltas in FORK.md |
| `2396af5` | `docs`: retire the WebDAV deployment, record the Local SMB vault source |
| `2f3d8e3` | `docs(hermes)`: record retiring the legacy rclone pieces |
| `9d8f6e7` | `feat!`: remove the WebDAV vault source (client, sync engine, scheduler, settings UI, tests) — the deployment serves a Local source over SMB now (see below) |
| `119ea8d` | `docs`: record the WebDAV removal commit in the delta table |
| `667c4f8` | `chore`: commit AGENTS.md (conflict resolved) and ignore machine-local agent tooling |

## Removal of the WebDAV vault source

**What was removed.** `VaultSource::WebDav` and everything that existed only to serve it: the
RFC-4918 client and the reconciling mirror sync engine (`src/vault/remote/`), `VaultWorkKind::WebDav`
and its dispatch arm (`dispatch_webdav_turn`), the per-Vault sync-turn scheduler
(`webdav_scheduler.rs`, threaded through `app_state`, `server` and `vault_runtime`), the registry
variant's validation / identity / poll-interval surface, the settings UI's "Add a Vault → WebDAV
endpoint" option and its edit-flow mapping, the WebDAV unit tests, and the `roxmltree` + `reqwest`
dependencies that only that client used. `REGISTRY_SCHEMA_VERSION` stays at 1: a registry that still
holds a `web_dav` Vault now fails to load with `unknown variant web_dav` — remove that entry, or
check out the commit before this one.

**Why.** A fork-only feature carries a standing tax: every upstream release merge re-integrates it
by hand into code upstream has restructured. The v2.6.1 merge is exactly that cost, itemised in
*Details of the upstream merge* below (the fork's WebDAV scheduler, its activation hooks in
`vault_runtime`, and its "a WebDAV vault is not git" arms all had to be re-applied to upstream's
newer files). The feature existed for a single deployment, and that deployment now gets the same
vault a simpler way: the Mac's Obsidian folder is exported over SMB, the host mounts it
(`/etc/fstab`), the container binds the mount, and Hatchdoor serves it as a plain `Local` source.

**What the switch bought, measured:** no Google OAuth credential on the host for the vault (it used
to expire on a 7-day cycle while the OAuth app sat in "testing"), no `rclone serve webdav` sidecar,
no second copy of the vault (the mirror), and no sync turn that walked ~253 directories one PROPFIND
at a time (~4 minutes per turn, giving ~8-11 minutes of effective freshness). **What it cost:** the
vault is now only as available as the Mac, and freshness is bounded by a 5-minute re-index timer,
because macOS's SMB server delivers no change notifications to Linux clients (measured: zero inotify
events for a round of Obsidian edits, while a stat rescan saw them within ~4 seconds). Both trade-offs
and their verification are in `HERMES.md` §12.

**What a Vault source is today:** `Local` (any directory, including a mounted network share),
`ExistingGit`, `ManagedGit`. Should an RFC-4918 source ever be wanted again, the design record is
`docs/architecture/work-packet-webdav-vaultsource.md` (marked SUPERSEDED) and the implementation is
in the commit before this one.

### Details of the upstream merge (`4e568cc`)

Merged upstream `main` at v2.6.1 (`c2b9861`) — 176 commits since `e631857`. Twelve files
conflicted; each was resolved by taking upstream's structure where it had restructured and
re-applying the fork's deltas into it:

- `Cargo.toml` / `Cargo.lock` — union of both sides (`reqwest` + `roxmltree` from the
  fork, `rmcp = "=3.1.4"` from upstream). The fork's security pin `h2 = 0.4.19` is kept:
  upstream's lock still carried 0.4.15, so the lock is reconciled with
  `cargo update -p h2 --precise 0.4.19`.
- `src/app_state.rs`, `src/server.rs`, `src/vault_runtime.rs`, `src/handlers/vaults.rs`,
  `src/mcp/routes.rs`, `src/mcp/tools/mod.rs` — both sides kept: upstream's per-Vault
  commit cooldown, running-version/`GIT_SHA` reporting and router changes, plus the
  fork's `WebDavScheduler`, `spawn_webdav_tick`, per-Vault webdav poll
  activation/deactivation, `dispatch_webdav_turn`, and the "a WebDAV vault is not git"
  arms.
- `src/vault_runtime/tests.rs` — upstream relocated the index tests to
  `vault_executor/tests.rs`; the relocation was taken and the fork's webdav tests kept.
- `AGENTS.md` — upstream's version (the machine-local agent scaffolding stays
  uncommitted, as always).
- `CHANGELOG.md`, `docs/architecture/module-map.md` — both sides' prose merged.

Verified after the merge: `cargo check`, `cargo check --tests`,
`node scripts/check-module-map.mjs` (213 production files, each owned exactly once),
frontend `npm run typecheck`, `vitest` (22 files / 393 tests), and upstream's
`verification` Docker target — `cargo test --locked` as a non-root user:
**1122 + 5 tests, 0 failed**.

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

### Details of the sync publish/heal commits (`d6d1f05`, `85b4f19`)

The reconciliation engine (`faaa77e`) only ever uploaded mirror files the remote did *not*
list, so a local edit to a note that already existed on the remote was stranded: its stored
remote fingerprint was unchanged, so nothing refreshed it, and it was not local-only, so
nothing pushed it. On the gdrive deployment six notes written by a documentation pass sat that
way — present in the mirror and the index, invisible to Drive and Obsidian — while every turn
logged `pushed=0` and looked converged. A local copy that had gone stale behind an unchanged
fingerprint was equally stuck.

`d6d1f05` decides a both-sides file whose remote fingerprint is unchanged per file
(`both_sides_action` in `src/vault/remote/sync.rs`):

- **push** when the mirror copy was modified after the last successful turn
  (`mtime > last_sync_at`) — the push rule the module doc always claimed, now applied to files
  the remote still lists instead of only to local-only files;
- **heal** when the copy was not modified since that turn but its bytes do not have the
  remote's size — a stale or partial copy, which the fingerprint gate can never see;
- otherwise leave the file alone.

`85b4f19` closes the window the heal arm opened: comparisons are strict on both sides, so a
copy whose mtime falls in the *same whole second* as the last turn's completion decides nothing
(no push, no heal). Such a copy is indistinguishable from a write that landed as the turn
finished, and healing it would silently revert that write — observed live, an MCP write
completed 28 ms after the turn that recorded `last_sync_at`, inside the same second.

Gates: `cargo check`, `cargo check --tests`, `node scripts/check-module-map.mjs` (213
production files owned), and the `Dockerfile.test` suite — **1123 + 5 tests, 0 failed** on both
commits. Deployed as `hatchdoor:local` with only the `hatchdoor` service recreated
(`hatchdoor:local-pre-d6d1f05` kept for rollback). Live verification: a stranded note was
pushed to Drive by the next turn (`pushed=1`), the three vault documentation notes followed
(`pushed=2`, plus the one-time refresh after a push), and the mirror, the Drive copy and the
fuse mount then agreed byte-for-byte on all 879 files.

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

## The reference deployment: a vault mounted over SMB

*(History of this section: it used to be titled "The gdrive build" and describe an `rclone-webdav`
sidecar serving `gdrive:MyObsidian` into a mirror. That path, and the fork code behind it, were
removed — see *Removal of the WebDAV vault source* above. What follows is what actually runs.)*

This fork exists because the deployment vault is edited on another machine, and
Hatchdoor (stock or the Docker Hub image) could not attach it directly. The fork added a
native **WebDAV `VaultSource`** for that — and has since **removed** it: the deployment
now attaches the same folder over an SMB mount as a plain `Local` source, which needs no
fork code at all (*Removal of the WebDAV vault source* above).

### Deployment (what runs today)

`/appdata/A--docker_stacks/Hatchdoor/docker-compose.yml` runs one container:

1. **`hatchdoor`** (image `hatchdoor:local`, built from THIS fork — the `build:`
   context is `/appdata/Hatchdoor`). HTTP on `:42824`, MCP on `/mcp`. The vault is
   bound in from the Mac Mini over SMB (`SMB_VAULT_PATH` → `/data/smb-vault`) — and
   `SMB_VAULT_PATH` is `/mnt/obsidian-vault/MyObsidian`, the share root, since the
   vault left the Google Drive domain on the Mac (2026-09-16). Changing it needs a
   container recreate, not a restart: a stale bind comes up healthy and indexes 0 notes.

The `rclone-webdav` sidecar that used to sit between Hatchdoor and Google Drive
(`rclone serve webdav gdrive:MyObsidian`, `WEBDAV_USER`/`WEBDAV_PASS`, the
`web_dav` vault `0851e3e7-…` and its local mirror) was retired on 2026-09-15. At that
point the served folder was the Mac's own Drive folder
(`/Volumes/Data/Google Drive/MyObsidian`), exported over SMB by macOS, and mounting
it directly removes the sidecar, the mirror, the Google OAuth token on this host, and
the per-directory PROPFIND walk (which cost minutes per sync turn). On 2026-09-16 the
vault itself moved one level up, out of the Drive domain, to `/Volumes/Data/MyObsidian`
(`/mnt/obsidian-vault/MyObsidian` over SMB) — the bind follows it. Hatchdoor now
registers that folder as `source: { type: local, path: /data/smb-vault }` — 730 notes,
read and written in place — and a host systemd timer re-indexes every 5 minutes,
because macOS SMB delivers this client no change notifications. The WebDAV code is not
merely idle here, it is **gone** (`9d8f6e7`, *Removal of the WebDAV vault source* above),
and the host-side Drive plumbing went with it: the `rclone-gdrive.service` fuse mount, the
`/mnt/gdrive` mountpoint, and the host `rclone` package and configs were all retired —
unit and configs parked under `/appdata/_retired-hatchdoor-drive-20260915/` on 2026-09-15,
the mountpoint directory deleted and the `rclone` package purged on 2026-09-16.
Re-introducing an RFC-4918 source means restoring both the code (the commit before
`9d8f6e7`) and that host tooling.

### MCP (what agents see)

Hatchdoor's embedded MCP server at `http://127.0.0.1:42824/mcp` exposes the
vault to agents — 35 tools, prefixed `mcp_hatchdoor_*` in Hermes (read/search/
create/edit/move/delete/archive notes, graph, tree, stats, vault admin,
attachments). It is registered in Hermes with its own bearer token
(`HATCHDOOR_MCP_BEARER_TOKEN`, synced into `~/.hermes/.env`, never committed),
and agents are *forced* through it: the `vault-maintenance`/`vaultagent` profiles
have the `file` and `code_execution` toolsets disabled, so the vault is only ever
touched via `mcp_hatchdoor_*`. That is the whole guard today: `approvals.deny` used to
block terminal commands naming the fuse path as well, and that rule was dropped on
2026-09-16 once the path itself was gone (mountpoint deleted, host `rclone` purged).
The complete playbook — registration, token sync, the real 35-tool surface,
operation templates, the hash-guard rule, the VaultAgent role, drift-detection
cron — is in [`HERMES.md`](HERMES.md).

### Operational caveats (live-verified 2026-09-02)

*Items 1-3 belong to the retired `rclone-webdav` sidecar and its mirror (both removed
2026-09-15) — kept as history, not as current behaviour. Item 4 is still the live
documentation convention.*

1. **MCP `move_note`/`delete_note`/`archive_note` do NOT propagate to Google
   Drive.** The sync engine is deliberately pull-side-only for deletions (root-404
   safety), so a move/delete changes only the mirror + index, and the next sync
   turn (~6.5 min) RESTORES the file from gdrive — observed reversion of a
   completed `move_note` within one turn. To make a delete/move stick: delete the
   file on the remote FIRST (curl `DELETE` to the WebDAV endpoint at
   `http://<sidecar>:42825/<urlencoded-path>` with `WEBDAV_USER`/`WEBDAV_PASS`),
   let one sync turn prune the mirror, then recreate at the target path if moving.
   `create_note`/`update_note` DO propagate (push side); only delete/move are
   pull-side-only.
2. **Direct writes to the mirror must be owned by the container user
   (uid 65532).** Content written to `<mirror>/...` as host root leaves root-owned
   files/dirs; the container's next pull/push fails EACCES on that subtree.
   Symptom: every turn logs `errors=2` with all other counters 0 and no
   error-level log line. Fix: `chown 65532:65532` the affected mirror paths.
3. **The rclone WebDAV sidecar caches directory listings** — files written to
   gdrive out-of-band (direct rclone/Drive API) can be invisible to PROPFIND
   until cache expiry. `docker restart rclone-webdav` clears it (stateless
   proxy; the vault tolerates one missed turn).
4. **Repo documentation layout convention:** per-repo docs live in the vault at
   `Homelab/02 Projects/<Repo>/` as `<Repo>.md` (project facts) + `Change Log.md`
   (history, newest first); container/deployment docs stay under
   `Homelab/02 Projects/Docker/<service>/`. Index entries use FULL-PATH wikilinks
   because note slugs collide across folders (`change-log-N`, `-readme` suffixes).

## Syncing with upstream

The fork is kept current by **merging** upstream into a branch and fast-forwarding `main`
once the gates pass (a rebase would rewrite the published fork history, and the fork's
deltas sit in files upstream also edits, so the conflicts have to be resolved either way):

```bash
git fetch https://github.com/BatterWorks/Hatchdoor main:refs/remotes/upstream/main
git tag pre-upstream-merge-$(date +%Y%m%d) HEAD          # rollback point
git checkout -b merge/upstream-<version>
git merge --no-ff --no-commit upstream/main              # resolve conflicts
#  - upstream wins where it restructured; re-apply the fork's WebDAV deltas into it
#  - Cargo.toml/Cargo.lock: union of both sides, then keep h2 pinned at 0.4.19
#  - never leave conflict markers; keep BOTH sides' tests
cargo check && cargo check --tests && node scripts/check-module-map.mjs
(cd frontend && npm run typecheck && npx vitest run)
git commit                                                # the merge commit
docker build --target verification -t hatchdoor:verify .  # cargo test, non-root
git checkout main && git merge --ff-only merge/upstream-<version>
git push origin main
```

Rebuild and redeploy the running deployment afterwards (`docker build` the image, then
`docker compose up -d`), and revert with `git reset --hard pre-upstream-merge-<date>` +
a rebuild if the merge proves bad.

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
