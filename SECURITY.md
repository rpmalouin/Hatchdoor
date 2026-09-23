# Security Policy

## Supported Versions

Security fixes target the current `main` branch and the latest published Docker
image, `battermanz/hatchdoor:latest`.

## Deployment Notes

Hatchdoor serves a private Markdown vault. Treat every network-exposed instance
as sensitive:

- Set `HATCHDOOR_WEB_BEARER_TOKEN` before binding to a non-loopback interface.
- Docker Compose binds Hatchdoor to `0.0.0.0` inside the container, so a web
  bearer token is required there.
- Enable MCP only for trusted clients and always set
  `HATCHDOOR_MCP_BEARER_TOKEN`.
- Keep the SQLite cache outside the Markdown vault. The cache is disposable, but
  the vault is the source of truth.
- If git sync is enabled, use a scoped HTTPS token and monitor
  `get_git_sync_status` for sync failures.

## Documentation: host-specific values are omitted

This repository is **public**, and its deployment documentation is written for that
audience. Where the reference deployment's own values would otherwise appear, the docs use
placeholders (`<repo>`, `<stack-dir>`, `<smb-mount>`, `<mac-ip>`, `<mac-vault-dir>`,
`<credentials-file>`, and similar), defined in the *Placeholders* note at the top of
[`FORK.md`](FORK.md) and [`HERMES.md`](HERMES.md).

That is deliberate, not missing information: **no credential value, LAN address, host
directory layout, or vault content from a real deployment is published here.** Operator-
specific values live with the operator — in the deployment's `.env`, its local notes, and
its vault — never in this repository. A `pre-push` hook in the maintainer's clone enforces
the same rule: it blocks a push whose added lines look like a token, password, private key,
`Bearer` value, RFC1918 address, or credentials path.

When reporting an issue that depends on such a value, describe the shape (which file, which
setting) rather than the value.

## Reporting Vulnerabilities

Open a private security advisory or contact the maintainer before publishing
details. Include affected version, deployment mode, reproduction steps, and
whether the issue can expose or mutate vault content.
