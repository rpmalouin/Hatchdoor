# Vault Refresh control (Web UI)

Status: DRAFT — implemented on `feature/vault-refresh-control`, in place for
review.
Author: agent work packet for the rpmalouin fork. Repo: `<repo>` (host paths are
not published).
Related: `docs/architecture/module-map.md`, `docs/architecture/interface-change-checklist.md`,
`docs/design/design-system.html` §05, `CHANGELOG.md` (Unreleased), `FORK.md`
(delta table).

## Outcome

The Web UI has a spelled-out, noticeable **Refresh** control in a row of its
own at the top of the explorer pane, above the Scope zone. Activating it asks
the server for a fresh index of the scoped Vault(s) — the existing
`POST /api/v1/vaults/{vault_id}/refresh` action — and then re-reads the
explorer tree and the changed-on-disk list. The control is present at every
Vault count, at every scope, on desktop and inside the mobile drawer — unlike
the Scope zone, which is absent at one enabled Vault and never rendered on
mobile — shows an in-flight state, names the scope it acts on, and stays on
screen disabled (with the reason) when the app cannot act. Nothing else
changes: the backend route and payload are untouched, and no polling or timers
are added.

This packet narrows the user-requested outcome. It does not authorize broader
work or opportunistic cleanup. Owned paths are writable only as necessary to
produce the outcome above.

## Boundaries

- Module: Application shell and navigation (`frontend/src/App.tsx`,
  `frontend/src/app/ExplorerPane.tsx`)
  Kind: composition/shared
- Module: Note editing and vault actions (`frontend/src/api/writeApi.ts`)
  Kind: product capability/adapter; safety-sensitive
- Module: Frontend API, authentication, and shared wire contracts
  (`frontend/src/types.ts` shared, not changed)
  Kind: infrastructure/shared contract

## Owned paths

- `frontend/src/api/writeApi.ts`
- `frontend/src/api/writeApi.test.ts`
- `frontend/src/app/ExplorerPane.tsx`
- `frontend/src/app/ExplorerPane.test.tsx`
- `frontend/src/styles/layout-explorer.css`
- `docs/design/design-system.html`
- `CHANGELOG.md`
- `FORK.md`
- `docs/user-vault/01 Get started/Browse and review through the Web UI.md`
- `docs/architecture/work-packet-vault-refresh-control.md`

## Public contract

Stable contract:

- `refreshVault(vaultId: VaultId, signal?: AbortSignal): Promise<VaultScheduleResponse>`
  exported from `frontend/src/api/writeApi.ts`. It issues
  `POST /api/v1/vaults/{vault_id}/refresh` through the existing `requestJson`
  path, treats `202 Accepted` as success, resolves the parsed
  `VaultScheduleResponse` (`{ vault_id, schedule: "queued" | "coalesced" }`),
  and otherwise throws the standard `WriteApiError`/`ConflictError` carrying the
  server's `code`.
- `ExplorerPane`'s new required prop
  `onRefreshVault: () => void | Promise<void>`, and the new
  `.explorer-refresh` (`ui-button`-based) control in the pane's own
  `.explorer-refresh-bar` row above the Scope zone.
- The Scope zone and its existing head, rows, retry wiring, contract, and every
  other prop are unchanged from `db26b01`.

Declared contract changes:

- Additive. A new frontend export, a new `ExplorerPane` prop, a new visible UI
  control, and new CSS selectors. No backend, wire-format, route, MCP, or
  `types.ts` change. No existing consumer behaviour changes.

## Coordination paths

- `frontend/src/App.write-mode.test.tsx` — one App-level integration test for
  the per-enabled-Vault refresh and the tree/recent re-read, following that
  file's full-app fetch-mock pattern.
- `frontend/src/App.tsx` — declared shell integration point: implements
  `handleRefreshVault`, calls `refreshVault` per enabled Vault sequentially,
  then `loadTree()` + `loadModifiedNotes()`, and surfaces a per-Vault failure in
  the existing polite notice strip.

## Packet record

- `docs/architecture/work-packet-vault-refresh-control.md` — this file.

## Consumed dependencies

- `src/handlers/vaults.rs::refresh_vault_handler` and
  `src/server.rs`'s demo-guarded `POST /api/v1/vaults/{vault_id}/refresh` route
  (read-only; not changed).
- `src/vault_management.rs`'s `VaultScheduleResponse` (serialized contract).
- `frontend/src/api/api.ts`'s `apiFetch` (timeout + abort signal handling).
- `frontend/src/hooks/useVaultTree.ts`'s `loadTree`/`loadModifiedNotes`.
- `frontend/src/hooks/useVaultScope.ts` and the collection client's enabled
  `vaults` list.
- `frontend/src/components/ui.tsx`'s `UiButton`.

## Forbidden paths and invariants

- Every Rust source under `src/` — the route already exists.
- `frontend/src/styles/base.css` tokens (no token changes; `palette.py` not
  triggered).
- No polling loops or timers for refresh; the collection revision stream already
  re-reads when the snapshot republishes.
- No new production modules or CSS framework; no raw hexes or new radii.
- ADR-01: Markdown stays authoritative and SQLite disposable — refresh only
  admits a server-side Index turn, it does not make the cache authoritative.
- ADR-03/11: HTTP mutations stay routed through `src/vault/write/`; this change
  adds no write path.

## Interface change checklist

Applicable items answered:

- Producing boundary and contract: `frontend/src/api/writeApi.ts` (Note editing
  and vault actions) produces the new `refreshVault` export; the pane
  (Application shell and navigation) produces the `onRefreshVault` prop and the
  top-row refresh control.
- Old contract: no frontend caller for the refresh route; the explorer pane had
  no refresh affordance.
- New contract: as in *Public contract* above. Additive.
- Why the existing contract cannot support the outcome: the route existed but
  had no TypeScript wrapper, and the pane had no control to reach it; the Scope
  zone, the first home considered, is absent at one enabled Vault and on mobile.
- Consumers searched for: `grep -rn "ExplorerPane" frontend/src` (only
  `App.tsx` and `ExplorerPane.test.tsx` render it); `grep -rn "writeApi"
  frontend/src` for existing importers. `App.tsx` already imports from
  `./api/writeApi`, so no new cross-module dependency edge is introduced.
- Compatibility / external consumers: additive frontend export and UI; no
  external contract changes; safe with any server that answers the route.
- Deployment / rollback: frontend-only; reverting the commit restores the
  previous UI. No migration.
- ADRs: ADR-01/03/11/05 invariants are untouched (contract checks recorded in
  *Forbidden paths and invariants*).
- Consumers/evidence: `App.tsx` is the only production consumer and is updated
  here; focused tests in `ExplorerPane.test.tsx`, `writeApi.test.ts`, and the
  App-level `App.write-mode.test.tsx` case.
- Module map: no module boundary, dependency, integration point, or focused
  validation changed in a way the structural checker tracks; no module-map edit
  required. `node scripts/check-module-map.mjs` run and green.
- Documentation: `CHANGELOG.md`, `FORK.md`, design system §05, and the Web UI
  reading note updated on this branch.

## Acceptance criteria

- The explorer pane renders a control whose visible label is the word
  `Refresh`, present at one enabled Vault, at several, and inside the mobile
  drawer.
- Clicking it calls `onRefreshVault`; while the call is in flight the label is
  `Refreshing…`, `aria-busy` is `true`, and the control is disabled.
- Its `title` names the scope (`Refresh All Vaults` / `Refresh <Vault name>`)
  and, when disabled by demo mode or write mode off, states that reason.
- Scope `all` admits one refresh per enabled Vault, sequentially; one Vault's
  failure does not stop the others and is surfaced in the polite notice strip.
- After the refreshes, the tree and changed-on-disk list are re-read.
- No polling or timers are added.
- The diff stays within owned paths, declared coordination paths, and this
  packet record; existing behaviour outside the outcome is unchanged.

## Validation

Focused:

```bash
cd frontend && npx vitest run src/app/ExplorerPane.test.tsx src/api/writeApi.test.ts src/App.write-mode.test.tsx
```

Full:

```bash
cd frontend && npm run typecheck && npm run test && npm run lint && npm run build
node scripts/check-module-map.mjs
just docs-freshness
```

Results recorded at hand-off: focused suites green; `typecheck` green; full
`npm run test` green; `lint` green; `build` green; module map OK;
`just docs-freshness` and the Web UI note review recorded in the hand-off
report.

Second round, re-homing the control to the pane's own top row: the same
focused suites plus the new one-Vault, several-Vault, and mobile cases are
green; the full gates and module-map check were repeated green, and the
`docs-freshness` review was re-run (both Web UI notes opened; the editor note
does not describe this control and did not drift).

## Escalation

If an undeclared path or interface is needed, stop expanding the diff and
classify the change:

- If it is necessary for the existing outcome and does not materially increase
  risk or authority, declare the path/change in this packet before editing it.
- If it would materially broaden the outcome, risk, or required authority, stop
  and ask the user before proceeding.

List affected consumers and complete the interface-change checklist whenever a
supported contract crosses its producing module boundary or is externally
observable.
