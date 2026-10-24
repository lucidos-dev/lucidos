# Dev runtime notes

Facts about the dev gateway and the client-update surface that no ADR, script
header or code comment holds. The rule is `.claude/rules/dev-runtime.md`.

## Gateway routing

The gateway reverse-proxies `/<slug>/` as a pure streaming forward. It strips
the slug, adds `X-Forwarded-Prefix: /<slug>/` and rewrites no body. The
reserved sigil namespace `/~/` holds the picker, the control API and health.
`/` redirects into the sole workspace, or serves the picker when there are
several.

The picker lists every workspace ever launched. A stopped workspace stays
listed and lazy-starts on a proxy hit or an explicit open. The gateway
provisions the Docker Postgres of a workspace created from the picker, in a
container named `lucidos-pg-gw-<id>`.

**Probe through the workspace prefix.** Use `https://<host>/<slug>/api/v1/...`.
A bare `/api/v1/...` reads as a workspace named `api` and answers
`unknown workspace 'api'`. An engine hit on its own port has base `/`, so the
bare form works there. The gotcha is gateway-only.

## Register a directory with a running gateway

POST `/~/api/v1/control/workspaces/adopt`. Never write `workspaces.json`
yourself: the gateway keeps its registry in memory and never re-reads the file.

| Field | Meaning |
|---|---|
| `dir` | Absolute path. Required. |
| `name` | Display name. Optional. |
| `autostart` | Boot-time start. Defaults to false. |
| `port` | Moves a stopped workspace's port. A 409 when the workspace runs, or another entry holds the port. |

The call registers only. It starts no engine and no Postgres, and writes
nothing inside `dir`. Re-adopting the same path keeps the picker-set name and
`autostart`. A same-slug adopt of a different path is a 409 naming both. The
response carries the entry's `port`: boot an engine there and the gateway adopts
it instead of spawning a second one on the same database.

Callers are `adopt_into_gateway` (`scripts/lib/workspace.sh`) and
`crates/lucidos-eval/src/gateway.rs`, which logs every failure as one line.

## Autostart and the legacy cluster

The per-workspace `autostart` flag governs gateway boot. The gateway re-adopts
running engines, spawns auto-start workspaces, and leaves the rest stopped. New
dev workspaces default to `autostart:false`. An explicit `web-dev.sh` launch
starts one for the session through the restart POST, so the launcher never
relies on gateway boot. An empty registry creates nothing.

A legacy per-workspace `lucidos-pg-<cksum>` cluster is migrated once. The
launcher dumps its `lucidos` database into `lucidos_<slug>`, verifies the target
and writes a marker under `.lucidos/`. The old container and volume stay.
`./scripts/decommission-legacy-postgres.sh -w <ws>` removes them, and refuses
without the marker and a reachable shared database.

## Gateway self-reload

A `--engine-only` Apply leaves the shared gateway on its compiled binary. A
change to `crates/lucidos-gateway/**` is rebuilt on disk but not served until
the gateway restarts. `crates/lucidos-app/index.html` counts as gateway source,
because the gateway `include_str!`s it for the boot splash
(`proxy.rs::app_splash_css`).

`GET /~/api/v1/control/gateway/status` returns `{build_id, update_available,
packaged}`. `update_available` is true only when the on-disk binary is NEWER,
decided by git ancestry over the two build ids (`build_id.rs`). Plain
difference would let a re-exec walk the only gateway backwards onto an older
binary. `build_id.rs` is a hand-synced copy of the engine's `engine_version.rs`
helpers, since ADR 0014 forbids linking the engine. Keep the two in step.

`POST /~/api/v1/control/gateway/reload` makes the gateway re-exec itself onto
the on-disk binary. The picker shows a reload icon, badged on
`update_available`.

## The client-update surface

`syncClientUpdateFromBuild` compares the running bundle's `CLIENT_BUILD_ID`
with the served `sw.js` `BUILD_ID`. The engine serves only a client compatible
with itself, so a difference means a compatible newer client is served. The
badge shows while stale. The toast shows when stale and not dismissed, and the
two decouple only on dismiss. The doc comment on `syncClientUpdateFromBuild`
holds the full rule.

Three events run the check: the restart itself (mixed change),
`ServedFrontendAdvanced` (frontend-only Apply) and an SSE reconnect. The
`ChangeApplied` arm does nothing for it, and no timer re-checks it. The
"Engine restarted" toast has no action: an engine-only Apply leaves the served
client byte-identical.

### Debugging a missed toast

Settings → System → Versions shows both halves of the comparison as build ids.
**Client** is `CLIENT_BUILD_ID`, the build running right now. **Service worker**
is the active worker's `BUILD_ID`. Both read `dev` under a live dev server.

- **Id unchanged across an Apply:** the service worker never picked up a new
  build. Suspect the rebuild or the stamp.
- **Id changed, no toast:** suspect the toast logic, but only for a freshly
  served, never-dismissed build. A lone badge after a dismiss is intended.
- **Served `BUILD_ID` did not move on a mid-build rebuild:** that is the pinned
  snapshot working. It advances on a Switch, and on a frontend-only Apply
  through `engine::frontend_refresh`. A mixed change advances only on Switch.

The Client row is deliberately not the engine's CalVer. A baked CalVer froze at
bundle-build time while the engine version kept bumping, so the page showed two
disagreeing numbers that no reload fixed. `vite.config.ts` records why.
