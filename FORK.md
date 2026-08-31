# FORK.md

## What this fork is

`rpmalouin/Hatchdoor` is a fork of [`BatterWorks/Hatchdoor`](https://github.com/BatterWorks/Hatchdoor).
It tracks upstream `main` and carries one targeted delta on top: a dependency
security-hardening commit. No functionality is changed.

## Delta over upstream

| Commit | Change |
| ------ | ------ |
| `04fee5e` | `chore(deps)`: patch Rust and frontend dependency security advisories |

### Details of the security commit

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

## Syncing with upstream

```bash
git fetch upstream          # assumes upstream = https://github.com/BatterWorks/Hatchdoor
git rebase upstream/main    # keep the fork's single delta on top of upstream
git push --force-with-lease origin main
```

If upstream merges an equivalent security fix, this delta can simply be dropped:
`git reset --hard upstream/main` and archive this fork.

## Deliberately NOT included

None — this fork contains only the single committed delta above. No secrets,
credentials, or machine-local configuration are committed. Local, untracked
agent scaffolding (`.codebuddy/`, `.gemini/`, `.mcp.json`, etc.) is kept out of
version control.