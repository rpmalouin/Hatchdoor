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
- The vault is the **real Obsidian vault on the Mac Mini**, reached over SMB — no
  WebDAV, no rclone, no mirror. `/etc/fstab` mounts `//10.1.10.75/Data` at
  `/mnt/obsidian-vault` (cifs, `credentials=/etc/mac-smb-credentials` (0600),
  `uid=65532,gid=65532,noperm,file_mode=0770,dir_mode=0770,soft,_netdev,nofail`), and
  the stack binds the vault's folder into the container as `/data/smb-vault`
  (`SMB_VAULT_PATH` in the stack `.env` = `/mnt/obsidian-vault/MyObsidian`, the share
  ROOT — it sat at `Google Drive/MyObsidian` until the Mac-side relocation of
  2026-09-16). Hatchdoor registers it as a
  `local` source (vault id `15b3a89e-80eb-4e1a-a5c8-ddfec68b00b7`, name `vault`) and
  reads/writes those files in place: a write through MCP is visible in Obsidian on the
  Mac immediately, and deletes land in `.hatchdoor-trash/` inside the vault. The
  retired WebDAV path (`rclone-webdav` sidecar, `WEBDAV_USER`/`WEBDAV_PASS`, the
  `0851e3e7-…` web_dav vault, its mirror under `<STATE>/vaults/`) is history as of
  2026-09-15 — no Google OAuth token for the vault remains on this host: the legacy
  `${VAULT_PATH}:/data/vault` bind, the host `rclone-gdrive.service` fuse mount and both
  rclone configs were retired the same day (parked under
  `/appdata/_retired-hatchdoor-drive-20260915/` for a one-step restore; the `/mnt/gdrive`
  mountpoint directory was deleted and the host `rclone` package purged on 2026-09-16, so
  a restore now also needs `apt install rclone`). macOS SMB
  sends this client no change notifications, so a systemd timer re-indexes every five
  minutes (§12).
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
`curl -s -D /tmp/h -X POST http://127.0.0.1:42824/mcp -H "Authorization: Bearer <TOKEN>" -H 'Content-Type: application/json' -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"probe","version":"1"}}}'`
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
`list_vaults` (this deployment: `15b3a89e-80eb-4e1a-a5c8-ddfec68b00b7`, the Local/SMB
vault; the WebDAV-era id `0851e3e7-2daf-4e73-aff2-f074f282c5c6` is retired).

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

The vault must never be touched through the filesystem. The only layer left is the
profile:

- The vault-maintenance profile has the `file` and `code_execution` toolsets
  disabled, so `read_file`/`write_file`/`patch`/`search_files` do not exist
  there at all. **This is what protects the live vault** — the SMB vault path is
  writable and nothing else gates it.

History: a second layer, `approvals.deny = ['*/mnt/gdrive*', '*rclone* gdrive*']`,
used to block terminal commands naming the retired fuse path — or rclone against the
gdrive remote — before execution, `--yolo` included. It was dropped on 2026-09-16
once the path itself was gone (`/mnt/gdrive` deleted, the host `rclone` package
purged; the `rclone-gdrive.service` unit and both rclone configs had been parked in
`/appdata/_retired-hatchdoor-drive-20260915/` on 2026-09-15). Nothing needs it back
unless the fuse path is ever resurrected.

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

Job `0a8f84d65e36` ("Vault drift detection + repair"): weekly `0 3 * * 1`,
`skills: ["vault-maintenance"]`, `enabled_toolsets: ["file"]` (file tools are
for reading the spillover cache and writing the report — vault access stays
MCP-only; cron layers the enabled MCP servers onto the per-job toolset list,
so the Hatchdoor surface remains available), `workdir` = the Hatchdoor fork.

**`code_execution` is BLOCKED in cron runs** regardless of `enabled_toolsets`
(there is no user present to approve the call), so a prompt that says "parse
the spillover with `execute_code`" sends the run into `BLOCKED:` retries.
Never instruct the job to use it: read spillover with `search_files` (regex)
+ `read_file` (offset/limit) instead.

Detection: `list_vaults` → `get_graph` + `get_note_links` (broken wikilinks,
orphans) → `recently_modified` + `get_stats` + `search_notes` (staleness).
Repair ONLY clear-cut cases (`move_rename_note` for broken links,
`edit_note` for stale links, `archive_note` for orphans — never delete);
ambiguous items go to "needs human review", not acted on. Prefer per-note
`get_note_links` over the whole-vault graph; when a response does spill to
`/root/.hermes/cache/spillover/`, read only the keys you need.

Tool-call hygiene (the live run lost time to all three): one entry per
`tool_call` — never batch MCP tools with local/file tools
(`Local tools require one entry per tool_call`); `get_stats` needs `scope`;
`get_note` needs `vault_id` + `slug`.

OpsBrain report contract (write every run, even when empty):
`/appdata/OpsBrain/logs/vault_drift_report.json`:
`{"timestamp": "<ISO-8601 UTC>", "attention": "actionable|low|none",
"findings": [{"path": "<note path>", "issue": "<description>"}]}`
Include repaired AND still-broken AND review items.

What OpsBrain actually does with it (`collector/vault_drift_ingest.py`, verified
against the live pipeline 2026-09-14):

- `findings` decides `up`: a present list (even empty) = up; a missing key = the
  source is reported down. Always write the key.
- **Categories are derived from the free-text `issue`, not from a field you set.**
  `"broken"` → `broken_links`; `"orphan"` → `orphans`; `"stale"`/`"metadata"` →
  `stale_metadata`; anything else → `needs_review` (counted, never actionable).
  "broken" is matched BEFORE "link", so an orphan whose issue says "no inbound
  links" is not misread as a broken link — keep the wording precise. Each list is
  capped at 30 entries for the digest/dashboard.
- The report's own `attention` field is **informational only** — OpsBrain recomputes
  its own ladder: `stale_report` > `needs_review`/`actionable` (broken links or
  orphans) > `attention` (stale metadata) > `low` > `none`.
- Downstream: the collector's `vault_drift` block feeds the reasoner digest, the
  dashboard (`source: "hatchdoor"`, counts + one-line summary) and a **notify-only**
  `notify_vault_drift` action. OpsBrain never writes to the vault.
- **Timestamp format decides whether the freshness guard works at all.** OpsBrain
  computes `age_s`/`stale` with `datetime.fromisoformat`, which on Python 3.10
  REJECTS a trailing `Z` (it throws, `age_s` stays null, `stale` stays false, and the
  whole `max_age_s` window is silently inert). Emit an explicit offset —
  `2026-09-14T03:00:02+00:00` — not `…T03:00:02Z`. The producer still emits `Z` today
  (§11); until that changes, do not rely on OpsBrain noticing a missed run.
- Cadence context: the job is weekly (`0 3 * * 1`), so a report up to ~7 days old is
  NORMAL — OpsBrain's window is 8 days (`sources.vault_drift.max_age_s: 691200`)
  before it calls the source stale.

The gateway is the scheduler: jobs never fire unless
`hermes gateway install && hermes gateway start` (systemd user service,
enable linger). Verify with `hermes gateway status`; after a `hermes update`,
run `hermes gateway restart` — until you do, cron has no way to know the
gateway is running pre-update modules ("mixed sys.modules", reported by
`hermes cron doctor`). Alerting for this job is covered in §11.

## 8. Fork fixes and rebuild (hatchdoor:local)

The fork `/appdata/Hatchdoor` is now a thin delta over upstream: a dependency
security-hardening commit plus `cecadd1`'s two fuse-vault write/index fixes —
which is exactly what a vault served from a mounted network share needs:

- `rename_exchange` (src/vault/write/fs_ops.rs) falls back to a three-rename
  emulation when `renameat2(RENAME_EXCHANGE)` is unsupported
  (EINVAL/ENOSYS/EOPNOTSUPP — fuse/rclone, NFS). Without it every
  hash-guarded write/move fails with `os error 22`.
- `populate.rs` deletes a row that owns a note's slug at a different
  `relative_path` before the upsert, so parallel-tree slug families
  (5-6x `_Inbox`/`_Areas`/`Home` basenames) no longer abort the index build
  with `UNIQUE constraint failed: notes.slug`.

Since the merge commit `4e568cc` the fork tracks **upstream v2.6.1**, and the
deltas above are re-integrated into upstream's newer structure on each merge.
(Historical note: the WebDAV deltas used to need the same treatment — a scheduler
spawned/aborted in server startup, per-Vault poll activation in `vault_runtime`, a
"not git" arm — which is the merge cost that motivated removing the feature, below.)
Rebuild procedure (source repo: `/appdata/Hatchdoor`, stack:
`/appdata/A--docker_stacks/Hatchdoor`):

```
cd /appdata/A--docker_stacks/Hatchdoor
docker compose build && docker compose up -d --force-recreate
```

Upstream builds now require **BuildKit** (current Docker uses it by default) and accept a
`GIT_SHA` build arg that makes the running build report `2.6.1 (dev abc1234)` in the
startup log and MCP `serverInfo`; the same Dockerfile carries a `verification` target that
runs `cargo test --locked` as a non-root user — use it as the gate after any merge:
`docker build --target verification -t hatchdoor:verify .` (the host cannot link the test
binary: pre-existing `ort`/`__isoc23` glibc issue).

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
- **A blocked context file is silent.** Any auto-loaded context file
  (`HERMES.md`, `AGENTS.md`, `.cursorrules`, `SOUL.md`) is dropped **whole**
  from the prompt when a single line matches a threat pattern — scope
  `context` blocks rather than warns, and nothing else fails loudly. This file
  was dropped from every cron run for a week because of one sample line: a
  probe written as a curl command whose `Authorization` header was built from
  a shell variable (dollar sign + a name ending in `TOKEN`) on the same line.
  `agent.log` said so plainly (`Context file HERMES.md blocked: exfil_curl`)
  while the job kept running without its repo context. Write such variables in
  docs without the leading dollar sign (e.g. `<MCP_TOKEN>`) and never put a
  real token in a tracked file. Check any context file with
  `agent.prompt_builder._scan_context_content(text, "HERMES.md")`: it returns
  the content, or a `[BLOCKED: …]` marker.
- **get_note params**: `vault_id` + `slug` (not `scope`).
- The job profile has no clock tool — anchor report timestamps to a known
  source if precision matters. When you do write one, make it parseable by
  OpsBrain: `datetime.fromisoformat` on Python 3.10 accepts
  `2026-09-14T03:00:02+00:00` but NOT the `…Z` form, and an unparseable stamp
  silently disables OpsBrain's staleness window (§7).
- Never put the bearer token in any committed file; it lives in the
  container env and `~/.hermes/.env` only.

## 10. Verification checklist

```
hermes mcp ls                          # hatchdoor enabled
hermes mcp test hatchdoor              # ✓ Connected + 35 tools
hermes -p vaultagent mcp test hatchdoor # role agent reaches the server
hermes -p vaultagent tools list        # no file/terminal/web toolsets
hermes gateway status                  # active (cron fires)
hermes cron list                       # job, schedule, next run, last status
hermes cron doctor                     # dispatch/delivery config health
curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:42824/mcp   # 401
```
End-to-end write test (proves the fuse write fix): `create_note` a scratch
note → `get_note` (hash matches) → `delete_note` (trashed).

Prove the OpsBrain hand-off (drift job → report → collector), read-only:
```
# what OpsBrain derives from the report right now
python3 - <<'PY'
import sys; sys.path.insert(0, "/appdata/OpsBrain")
from collector.vault_drift_ingest import pull_report, classify, create_context_nodes
r = pull_report("/appdata/OpsBrain/logs/vault_drift_report.json", max_age_s=691200)
print("up:", r.get("up"), "age_s:", r.get("age_s"), "stale:", r.get("stale"), "err:", r.get("err"))
c = classify(r); print("counts:", c["counts"])
print("attention:", create_context_nodes(r, c)["attention"])
PY
```
`age_s: None` on a report that has a timestamp means the stamp was unparseable
(`…Z`) — the staleness guard is off, not "fresh" (§7).

## 11. Alerts when the job fails (cron delivery)

`deliver: local` only saves output. `failure_deliver: "bot-chat"` sends
**failures** into the profile's canonical Bot Chat as a message the bot
replies to — not a passive notification: it starts an agent turn with full
tools that will investigate (tens of API calls per failure, and it may act).
`bot-chat` needs no messaging platform: `cron/scheduler_preflight.py` skips
bot-chat targets, so it works on a host where nothing is connected. Before
pointing `deliver` at a platform name, confirm one exists —
`~/.hermes/gateway_state.json` (`"platforms": {}` means none) and
`~/.hermes/channel_directory.json` (0 targets).

Which failures alert:

| failure | recorded as | alert? |
|---|---|---|
| a run raises (script non-zero exit, agent crash, timeout) | execution row + `last_error` | **yes** (`_deliver_crash_failure`) |
| dispatch/fire fails (`Restart-safe cron worker dispatch failed: …`) | `last_fire_error` + execution row, visible via `hermes cron doctor` / `hermes cron incidents` | **no** |

A missed fire — the class that hit this job on 2026-09-07 — is therefore
silent. The intended backstop is OpsBrain's `sources.vault_drift` window
(`max_age_s`, 8 days): a stale report is meant to surface as
`attention: stale_report` plus a `notify_vault_drift` "Vault-drift report is
stale; cron may not have run" notification, repeated **every** collector cycle
while the flag is set (unlike the GPU notice, it has no once-a-day cap).

That backstop is currently inert, and the reason is worth knowing: OpsBrain's
`pull_report` parses the report `timestamp` with `datetime.fromisoformat`, which
Python 3.10 rejects for the trailing-`Z` form the producer writes. Parsing throws,
`age_s` is left `null`, `stale` is left `false` — so a missed Monday run is NOT
detected today. Fix on either side: emit `+00:00` instead of `Z` in the report
(preferred, §7), or make the reader accept `Z`. Verify with the read-only probe
in §10 — `age_s: None` next to a timestamp means the guard is off.

Prove a lane end-to-end rather than trusting the config: create a throwaway
`no_agent` job whose script exits non-zero, give it
`failure_deliver: "bot-chat"`, fire it, and watch for a
`hermes chat -c Bot Chat --create-if-missing` process plus a new "Bot Chat"
session. Remove the job and its script afterwards.

## 12. Keeping the SMB-sourced vault fresh (systemd timer)

macOS's SMB server delivers **no change notifications** to this Linux client —
measured 2026-09-15 with a recursive inotify watch armed on all 253 directories of
the mount: a round of Obsidian edits on the Mac produced 0 events, while an
independent stat rescan saw the new sizes within ~4 s (positive control: a write made
*through* the mount fired `MODIFY|CLOSE_WRITE` immediately, so the watches were
correct). Hatchdoor's watcher is inotify-only (`notify::RecommendedWatcher` in
`src/vault_watcher.rs`, no polling fallback), and a `Local` Vault has no periodic
re-index of its own — `VaultWorkKind::Index` is requested only by the watcher, at
startup, and by explicit API/MCP calls. Without a poll, the note tree and search go
stale on the first Mac-side edit, even though note *content* reads stay correct
(ADR-01 re-scan).

Installed fix (host-side, not in the repo):

- `/usr/local/bin/hatchdoor-vault-refresh` — reads the web bearer token from the stack
  `.env` itself, verifies `/mnt/obsidian-vault` is mounted (mounting it from fstab when
  it is not), then `POST /api/v1/vaults/<id>/refresh` for every enabled, active Vault.
  202 = admitted to the index FIFO. It never prints a credential.
- `/etc/systemd/system/hatchdoor-vault-refresh.{service,timer}` — `OnCalendar=*:0/5`,
  `AccuracySec=30s`, `RandomizedDelaySec=20s`, enabled and running. One pass costs
  ~41 ms of client CPU plus a sub-second scan of ~730 notes — cheaper *and* fresher
  than the old WebDAV path (300 s poll + a ~4 min PROPFIND walk, ~8-11 min effective).

Measured behaviour (2026-09-15): a note written through a **second** cifs mount of the
same share (separate SMB session, so the container gets no event — the Mac-edits case)
appeared in `get_tree`/`search_notes` within one tick (~5 min), and a delete through
that same path dropped from the index on the next pass. Rate change: edit the timer's
`OnCalendar` split, then `systemctl daemon-reload && systemctl restart
hatchdoor-vault-refresh.timer`. Watch it with
`journalctl -u hatchdoor-vault-refresh.service` and
`docker logs hatchdoor | grep 'Search index ready'`.

Pitfall: the index FIFO is shared across Vaults, so a long first index (a new Vault
re-embeds everything — ~10 min for 730 notes / 2,458 chunks, measured 10m01s after the
2026-09-16 repoint) delays a refresh request;
the request is coalesced, not lost. Judge progress by the `Indexing: N of M notes`
lines rather than by a missing `Search index ready`. Incremental passes are cheap
(1 changed note → 24 chunks in ~6 s).

### Repointing the vault folder (when the vault moves on the Mac)

1. Set `SMB_VAULT_PATH` in the stack `.env` to the new folder inside the share.
2. **Recreate** the container: `docker compose up -d` from the stack dir. A plain
   `docker restart` keeps the OLD bind — the container comes up healthy and serves
   the old path.
3. The failure mode is silent: with a missing/empty bind target Hatchdoor logs no
   error and reports `note_count: 0` while `search: ready`. Check `get_stats` for the
   real count (this vault: ~730) instead of trusting the healthcheck.
4. The repoint forces a full re-embed; `search` reads `indexing` and
   `capabilities.search` stays `false` until it finishes (~10 min above). Reads of
   note *content* work throughout.
5. Verify the WRITE path, not only reads: `create_note` a throwaway note, confirm the
   file appears under the new host path only, then `delete_note` (it lands in
   `.hatchdoor-trash/`, which is normal — say so rather than hiding it).
6. The Vault registry (`state/vaults.json`, `source: {type: local, path:
   /data/smb-vault}`) needs no change: the container-side path is unchanged by a
   host-side move.
