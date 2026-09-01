# FORK.md

## What this fork is

`rpmalouin/Hatchdoor` is a fork of [`BatterWorks/Hatchdoor`](https://github.com/BatterWorks/Hatchdoor).
It tracks upstream `main` and carries a small, documented set of deltas on top:
a dependency security-hardening commit and native WebDAV vault support. Each
delta is listed below; everything else tracks upstream unchanged.

## Delta over upstream

| Commit | Change |
| ------ | ------ |
| `04fee5e` | `chore(deps)`: patch Rust and frontend dependency security advisories |
| `02dd552` | `feat(webdav)`: add a WebDAV `VaultSource` backend, client, and sync engine |
| `5443413` | `feat(webdav)`: wire WebDAV into the work scheduler, server dispatch, and settings UI |
| `e98863f` | `fix(webdav)`: WebDAV sync-turn scheduler — the missing Phase D trigger (see below) |

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

### Details of the WebDAV scheduler commit

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
- **`src/vault_runtime.rs`**: after a successful WebDAV sync, the dispatch
  re-publishes local-content availability (`publish_local_content_after_sync`,
  the same stat + `set_local_content_status` the managed-Git success path
  uses), so the live snapshot flips to `activation: Active` (browse + mutate)
  the moment the mirror exists — no reconcile or restart required.

Verified: `cargo check` and `cargo check --tests` clean; module-map gate passes;
deployed live — a WebDAV-sourced vault self-heals from missing-mirror to
indexed-and-browsable without any manual API call.

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
never committed. Local, untracked agent scaffolding (`.codebuddy/`, `.gemini/`,
`.mcp.json`, etc.) is kept out of version control.