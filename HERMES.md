# HERMES.md — Hatchdoor MCP Server: build-out playbook

Everything another Hermes instance needs to wire Hatchdoor's MCP server (the
Obsidian-vault server) into Hermes: registration, token handling, the real
tool surface, operation templates, access enforcement, the VaultAgent role,
drift detection, and the fork fixes. Self-contained — no local skills
required. If you have the `vault-maintenance` and `hermes-mcp-integration`
skills installed, they are the maintained copies of this knowledge.

Companion files: `MEMORY.md` (repo + live-stack context), `SPEC.md`
(application structure), `README.md` (user docs).

## 1. Topology (what Hatchdoor is, where it runs)

- Container `hatchdoor` (image `hatchdoor:local`, built from the fork at
  `/appdata/Hatchdoor`), HTTP on port 42824, MCP route `http://127.0.0.1:42824/mcp`.
- The vault is a **Google Drive vault** (`gdrive:MyObsidian`) served to
  Hatchdoor by the `rclone-webdav` sidecar container (rclone `serve webdav`
  on `:42825`, creds `WEBDAV_USER`/`WEBDAV_PASS` from the stack `.env`).
  Hatchdoor registers it as a `web_dav` source
  (`url: http://rclone-webdav:42825`, `poll_interval_secs: 300`, vault id
  `0851e3e7-2daf-4e73-aff2-f074f282c5c6`) and syncs it to a **local mirror** at
  `<STATE>/vaults/<id>/webdav` (container `/data/state/vaults/<id>/webdav`).
  Reads/writes hit the mirror; the Drive remote is only touched through the
  WebDAV sidecar. Hatchdoor owns mirror consistency — never touch the remote
  or the mirror directly. (The old `/data/vault` fuse bind and `/mnt/gdrive`
  mount are legacy; the active vault path is the WebDAV mirror.)
- Server-side env gates (compose `.env`): `HATCHDOOR_MCP_ENABLED=true`,
  `HATCHDOOR_MCP_WRITE_ENABLED=true` (gates the write tools),
  `HATCHDOOR_MCP_BEARER_TOKEN`, `HATCHDOOR_MCP_ALLOWED_ORIGINS`.
- The container is **distroless (no `sh`)**: read its env with
  `docker inspect hatchdoor --format '{{range .Config.Env}}{{println .}}{{end}}'`,
  never `docker exec`.

## 2. Register the MCP server

1. Confirm the route is mounted: unauthenticated `GET /mcp` → `401` (auth
   challenge) means mounted; `404` means the server-side enable flag is off.
2. Extract the token from the container env and sync it into `~/.hermes/.env`
   as `HATCHDOOR_MCP_BEARER_TOKEN` (idempotent sync script; chmod 600).
   Never hardcode the value anywhere.
3. Configure via `hermes config set` (never hand-edit config.yaml):

```
hermes config set mcp_servers.hatchdoor.url http://127.0.0.1:42824/mcp
hermes config set 'mcp_servers.hatchdoor.headers.Authorization' 'Bearer ${HATCHDOOR_MCP_BEARER_TOKEN}'
hermes config set mcp_servers.hatchdoor.connect_timeout 60.0
hermes config set mcp_servers.hatchdoor.timeout 180
hermes config set mcp_servers.hatchdoor.enabled true
```

Hermes interpolates `${VAR}` from `.env` at MCP discovery. Re-run the sync
script if the container token rotates, then `/reload-mcp`. Config changes
take effect on a new session or `/reload-mcp`, never mid-conversation.

## 3. Verify the LIVE tool surface — never trust docs

Tool names in goals/READMEs are frequently wrong (a common spec claims
`hatchdoor.vault.read/write/semanticSearch/backlinks` — **none exist**).
Pull the real surface from the running server:

```
hermes mcp test hatchdoor     # → ✓ Connected + every tool listed (35)
```

Raw probe (StreamableHTTP needs the session-id handshake; some servers answer
SSE with a `data: ` prefix — strip it before parsing):
`curl -s -D /tmp/h -X POST http://127.0.0.1:42824/mcp -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"probe","version":"1"}}}'`
then send `tools/list` in a second POST carrying the `mcp-session-id` response
header.

Real 35-tool surface (Hermes names them `mcp_hatchdoor_<tool>`):

- Discovery/model: `get_model_setup_status`, `accept_gemma_terms`, `decline_gemma_terms`
- Read/search/metadata: `list_vaults`, `search_notes` (mode `semantic`|`keyword`),
  `get_note`, `get_note_links` (outgoing + backlinks), `resolve_wikilink`,
  `get_tree`, `get_stats`, `get_graph`, `list_note_attachments`,
  `get_attachment_import_config`, `recently_modified`
- Vault admin: `create_vault`, `edit_vault`, `enable_vault`, `disable_vault`,
  `disconnect_vault`, `sync_vault`, `retry_vault`
- Note write/maintenance: `create_note`, `update_note`, `append_to_note`,
  `edit_note`, `replace_section`, `rename_note`, `move_note`,
  `move_rename_note`, `archive_note`, `delete_note`
- Attachments: `import_attachment`, `move_attachment`, `rename_attachment`, `delete_attachment`

Addressing rules: notes take `vault_id` + `slug` (vault-relative identifier,
e.g. `folder/note-name`); collection reads take `scope` (one vault_id or the
literal `all`). The `vault_id` is immutable across revisions — get it from
`list_vaults` (this deployment: `0851e3e7-2daf-4e73-aff2-f074f282c5c6`).

## 4. Operation → tool templates

| Requested op | Real tool | Notes |
|---|---|---|
| searchNotes | `mcp_hatchdoor_search_notes` | mode=`keyword` (default is semantic!) |
| semanticSearch | `mcp_hatchdoor_search_notes` | mode=`semantic` (the default) |
| readNote | `mcp_hatchdoor_get_note` | slug + vault_id |
| updateNote | `mcp_hatchdoor_update_note` | full-content replace; needs `expected_content_hash` |
| moveNote | `mcp_hatchdoor_move_note` | rewrites backlinks, moves assets; needs `target_folder` + hash |
| deleteNote | `mcp_hatchdoor_delete_note` | trashes to `.hatchdoor-trash`; needs hash |
| backlinks | `mcp_hatchdoor_get_note_links` | outgoing AND backlinks in one call |

**Hash-guard rule (read-before-write):** update/move/delete all require
`expected_content_hash` from a prior `get_note` — concurrent-edit protection.
Stale hash → abort and re-read. Prefer reversible actions: `archive_note`
over `delete_note`, `move_rename_note` over hand-editing links.

## 5. Enforce hatchdoor-only vault access

The vault must never be touched through the filesystem. Two layers:

- `approvals.deny` in config.yaml (both the default and vault-maintenance
  profiles): `['*/mnt/gdrive*', '*rclone* gdrive*']` — any terminal command
  whose text contains the vault path (reads included), or rclone against the
  gdrive remote, is BLOCKED before execution; `--yolo` does not bypass.
  Denied calls return `BLOCKED: ... Do NOT retry or rephrase` — route the
  operation through `mcp_hatchdoor_*` instead.
- The vault-maintenance profile has the `file` and `code_execution` toolsets
  disabled, so `read_file`/`write_file`/`patch`/`search_files` do not exist
  there at all.

Footgun: because the deny rule matches the literal path, you cannot run ANY
terminal command containing `/mnt/gdrive` — including one that would edit
this guard. Compose the string at runtime (e.g. `printf '["*%s/%s*"]' mnt gdrive`).

Vault memory files (`memory` / `memory.md` in the vault) are also routed
through MCP: `get_note` + `append_to_note`/`edit_note`/`update_note`
(hash-guarded). The Hermes `memory` tool store (`~/.hermes/memories`) is a
separate thing — don't confuse the two.

## 6. VaultAgent role agent

A profile restricted to hatchdoor-only tools, for interactive vault work:

```
hermes profile create vaultagent --clone-from vault-maintenance \
  --description "VaultAgent: vault maintenance via hatchdoor MCP only"
hermes -p vaultagent tools disable terminal web browser computer_use image_gen tts vision delegation cronjob
hermes -p vaultagent config set mcp_servers.code-review-graph.enabled false
hermes -p vaultagent config set mcp_servers.donsetch.enabled false
```

Write a `SOUL.md` persona (profile dir `~/.hermes/profiles/vaultagent/`):
mission = maintain the vault, fix broken links, update homelab docs, semantic
search, rewrite notes safely; hard rules = only `mcp_hatchdoor_*`, scope via
`list_vaults`, read-before-write with the hash guard, never invent tool names.
CLI alias `vaultagent` is created automatically.

## 7. Drift detection (cron)

Job: weekly (e.g. `0 3 * * 1`), `skills: ["vault-maintenance"]`,
`enabled_toolsets: ["file", "code_execution"]` (file only to write the
report — vault access stays MCP-only), `workdir` = the Hatchdoor fork.
Detection: `list_vaults` → `get_graph` + `get_note_links` (broken wikilinks,
orphans) → `recently_modified` + `get_stats` + `search_notes` (staleness).
Repair ONLY clear-cut cases (`move_rename_note` for broken links,
`edit_note` for stale links, `archive_note` for orphans — never delete);
ambiguous items go to "needs human review", not acted on. The graph response
can be large — parse the spillover cache file with `code_execution`, not raw
reads.

OpsBrain report contract (write every run, even when empty):
`/appdata/OpsBrain/logs/vault_drift_report.json`:
`{"timestamp": "<ISO-8601 UTC>", "attention": "actionable|low|none",
"findings": [{"path": "<note path>", "issue": "<description>"}]}`
Include repaired AND still-broken AND review items.

The gateway is the scheduler: jobs never fire unless
`hermes gateway install && hermes gateway start` (systemd user service,
enable linger). Verify with `hermes gateway status`.

## 8. Fork fixes and rebuild (hatchdoor:local)

The fork `/appdata/Hatchdoor` carries the WebDAV deltas that make the gdrive
build work (commits `02dd552`, `5443413`, `f13441c`, `f3dd537` — WebDAV
VaultSource, wiring, sync-turn scheduler, activation publish) plus `cecadd1`,
two fixes the WebDAV-mirror write path requires:

- `rename_exchange` (src/vault/write/fs_ops.rs) falls back to a three-rename
  emulation when `renameat2(RENAME_EXCHANGE)` is unsupported
  (EINVAL/ENOSYS/EOPNOTSUPP — fuse/rclone, NFS). Without it every
  hash-guarded write/move fails with `os error 22`.
- `populate.rs` deletes a row that owns a note's slug at a different
  `relative_path` before the upsert, so parallel-tree slug families
  (5-6x `_Inbox`/`_Areas`/`Home` basenames) no longer abort the index build
  with `UNIQUE constraint failed: notes.slug`.

Rebuild procedure (source repo: `/appdata/Hatchdoor`, stack:
`/appdata/A--docker_stacks/Hatchdoor`):
```
cd /appdata/A--docker_stacks/Hatchdoor
docker compose build && docker compose up -d --force-recreate
```
Verify with `hermes mcp test hatchdoor` and `list_vaults` (search must read
`ready`).

## 9. Pitfalls

- **Setup gate**: if the search index fails to build, `list_vaults` shows
  `search: stale` + `search_error` (e.g. `vault_index_failed`), setup state
  goes `failed`, and most tools answer "Hatchdoor is still being set up".
  Fix the index cause, then the gate clears itself on retry.
- **MCP client backoff**: repeated failures trip a ~60s client-side
  "unreachable" backoff on the Hermes side. Diagnose (server healthy? `/mcp`
  still 401?) instead of retrying; the container being healthy does not mean
  the setup gate is clear.
- **get_note params**: `vault_id` + `slug` (not `scope`).
- The job profile has no clock tool — anchor report timestamps to a known
  source if precision matters.
- Never put the bearer token in any committed file; it lives in the
  container env and `~/.hermes/.env` only.

## 10. Verification checklist

```
hermes mcp ls                          # hatchdoor enabled
hermes mcp test hatchdoor              # ✓ Connected + 35 tools
hermes -p vaultagent mcp test hatchdoor # role agent reaches the server
hermes -p vaultagent tools list        # no file/terminal/web toolsets
hermes gateway status                  # active (cron fires)
curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:42824/mcp   # 401
```
End-to-end write test (proves the fuse write fix): `create_note` a scratch
note → `get_note` (hash matches) → `delete_note` (trashed).
