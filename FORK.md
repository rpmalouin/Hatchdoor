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