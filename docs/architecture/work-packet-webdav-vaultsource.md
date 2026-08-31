# WORK PACKET — WebDAV VaultSource for Hatchdoor

Status: DRAFT — for review before shared-file changes.
Author: rpmalouin fork. Repo: /appdata/Hatchdoor.
Related: FORK.md (fork delta), ADR-01/03/10/11 (constraints).

## Outcome

Hatchdoor can attach a Vault whose authoritative content lives on a WebDAV
server (e.g. `rclone serve webdav` on a Mac Mini, or any RFC-4918 endpoint),
browse/search it, and write notes back — as a first-class `VaultSource::WebDav`.

## Critical design fact (non-negotiable): ADR-01 governs reads

`src/cache/vault_snapshots.rs:37-39` documents a standing invariant:

> "This is intentionally a cache-local representation: callers must treat it as
> disposable data and keep exact note reads on the authoritative Markdown path."

The SQLite snapshot is a **disposable read model** and may be arbitrarily stale.
`exact_note` therefore **must not** serve note *content* from the cache — the
current `authoritative_index()` (re-scan + live file read) is **intentional**,
not a bug. My earlier idea to "serve exact-note reads from cache" is **cancelled**:
it would violate ADR-01 (Markdown authoritative, SQLite disposable).

**Consequence:** a WebDAV source cannot cheaply read note content over the
network per request. The faithful architecture reuses Hatchdoor's **ManagedGit
pattern**: a remote-backed source keeps a **local mirror checkout** that IS the
authoritative read path; the index, watcher, and atomic write layer all run on
the mirror; a background **sync turn** propagates remote↔local. This preserves
every ADR-01/03/10/11 invariant and needs no new write model.

## What the codebase requires

1. **`VaultSource` is a tagged enum** (`src/vault_registry.rs`,
   `#[serde(tag="type", rename_all="snake_case", deny_unknown_fields)]`) with an
   explicit `REGISTRY_SCHEMA_VERSION` + migration. Adding `WebDav` touches the
   serialized registry, `source_vault_path`, `Debug`/redaction, validation,
   `add`/`edit`, `managed_git_poll_interval` accessor surface, and ~10
   `match source` sites.
2. **A local mirror checkout** under the state directory is the authoritative
   read path (like `ManagedGit`'s checkout). `vault_path()` resolves there;
   Local-style index, `fs::notify` watcher, and `vault/write/fs_ops.rs` atomic
   writes all run on the mirror unchanged.
3. **A WebDAV sync turn** (new `VaultWorkKind::WebDav` or reuse `Git` structure)
   polls the remote (`poll_interval_secs`, floor 60s, mirroring ManagedGit):
   pull remote→mirror (PROPFIND list + GET changed), push mirror→remote
   (PUT changed/created, DELETE removed), under the same mutation-lock
   discipline as `dispatch_managed_git_turn`. Conflicts resolved like ManagedGit
   export semantics (ADR-10: don't force-checkout over uncommitted edits).
4. **WebDAV client** — RFC-4918 (PROPFIND/GET/PUT/DELETE/MKCOL), Basic auth,
   minimal deps (add `reqwest` — already transitive via hf-hub — plus
   `roxmltree`/`quick-xml` for multistatus). No heavy WebDAV framework (ADR-06/13).

## Phases

- **Phase A — read-path "fix": CANCELED.** Serve-from-cache would break ADR-01.
  The real gdrive-read win is config (rclone `--vfs-cache-mode full`, applied).
- **Phase B — WebDAV client** (`src/vault/remote/mod.rs`): list/fetch/put/
  delete/mkdir, Basic auth, PROPFIND multistatus parsing + percent-decoding, URL
  encoding. **DONE** — live-verified against `rclone serve webdav` (PUT→GET→
  list round-trip, encoded filenames, auth).
- **Phase C — `VaultSource::WebDav` + mirror checkout**: **backend DONE** —
  registry variant (`url`, optional `vault_subdirectory`, `poll_interval_secs`),
  `#[serde(default)]` poll interval, URL validation (http(s), no inline creds,
  no query/fragment), remote-backed credentials acceptance, mirror path under
  state dir (`<state>/vaults/<id>/webdav`), identity comparison, and every
  `match VaultSource` site (registry, vault_runtime, server posture, git-dispatch
  no-op). Compiles; module-map green. **Frontend (settings UI) still pending.**
- **Phase D — WebDAV sync turn**: **sync engine DONE** (`src/vault/remote/sync.rs`,
  boxed-recursive pull + push-new, mirrors ManagedGit execution model). **LIVE-
  VERIFIED** against `rclone serve webdav`: pulled a 3-file/2-dir vault into an
  empty mirror, pushed new local notes + created a new directory remotely, and
  a fresh mirror re-pulled everything. **Work-kind dispatch/scheduler wiring
  still pending** (route WebDav through `VaultWorkCoordinator` + a poller like
  `ManagedGitScheduler`).
- **Phase E — write backend**: writes already run on the local mirror via the
  existing atomic `vault/write/` layer (no new write model); the sync turn's
  push-half propagates them up. **Pending: confirm propagation end-to-end.**

## First deliverable (this session)
Corrected design doc + Phase B client + Phase C backend + Phase D sync engine,
all compiling, with the client and sync live-verified against a real
`rclone serve webdav` endpoint. No ADR-01 change is proposed. Remaining:
Phase C settings UI, Phase D dispatch/scheduler wiring, Phase E write
confirmation.