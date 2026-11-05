# Dev runtime notes

Facts about the dev gateway and the client-update surface that no ADR, script
header or code comment holds. The rule is `.claude/rules/dev-runtime.md`.

## Gateway routing

The gateway reverse-proxies `/<slug>/` as a pure streaming forward. It strips
the slug, adds `X-Forwarded-Prefix: /<slug>/` and rewrites no body. The
reserved sigil namespace `/~/` holds the picker, the control API and health.
`/` redirects a credentialed caller into the sole workspace. It serves the
picker when there are several, and to a caller with no credential, so a
stranger never learns a slug from it.

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

### Disposable harness workspaces

A harness authenticates control calls with the machine-local token in
`x-lucidos-local-token`. For a **new** registration it sends `origin: "eval"`
(or `"e2e"`), `ephemeral: true`, and optionally `expires_at` as an RFC 3339 UTC
timestamp, alongside the adoption fields above. Expiry suggests a workspace in
manual review; it never schedules deletion.

A successful new ephemeral adoption returns `owner_token` beside `workspace`.
Persist that secret with owner-only permissions before treating registration as
complete. Keep it outside the disposable workspace so separate seed/run commands
and interrupted cleanup can reuse it. Do not log it or commit it to source.
`lucidos-eval` stores it atomically under `.cleanup-owners/` beside its arms.

A stable per-workspace `.lock` file serializes credential publication and run
binding across harness processes. Contention fails promptly for an explicit
retry; never unlink that lock file during credential rotation. Adoption refuses
an unarchived credential before contacting the gateway, so a reused label cannot
mint a new secret that the harness then fails to persist.
A missing token is a visible failure of cleanup readiness, never permission to
fall back to deleting files or dropping databases directly.

Observe the registry before adoption. An existing registration is not a new
harness-owned workspace: preserve its origin and protection. A local-token
caller cannot retag it as ephemeral, and cannot mint a replacement owner token.
Only a paired device can change an existing entry's protection.

Eval seeding exclusively creates a new directory and database. A familiar
`eval-` prefix, an old credential file, or an absent directory never authorizes
resetting an existing database or replacing data. With a configured gateway,
seeding first requires a successful registry observation proving the slug is
absent. A collision stops the seed: choose a fresh run label for a new run.

Resuming recorded task progress reuses the original arms without seeding them.
Before booting an arm, the run atomically pins its cleanup credential (or its
absence) in an owner-only file under the results directory's
`.cleanup-owners/<run>/`. A resume reads that immutable binding, never the current
credential beside a potentially reused workspace label. A legacy resumed arm
without a binding refuses to drive rather than inferring ownership. Retain these
bindings alongside the results; they contain secrets and must not be published.

A partial seed retains its resources for inspection; it never force-drops a
colliding database on retry.

An uncut eval run stops its engines and atomically publishes per-arm
`<run>-<arm>-<repeat>.snapshot.json` files beside its results. These preserve
scoring data files, thread observations, replay captures and call costs. Score,
report and replay use snapshots when present, so they remain usable after arm
removal. A corrupt or mismatched snapshot fails visibly; only legacy runs with
no snapshot fall back to live databases. Publication never replaces a snapshot.

A data symlink, special file or nonportable path refuses capture and retains
the arm. Cut runs retain their live inputs for resumption.

Successful uncut runs then delete their pinned, owned arms by default.
`run --keep-workspaces` retains them. Failed and cut runs retain their arms.
Before any DELETE, every arm must have a validated durable snapshot and its
original ownership binding. Arms pinned without an owner remain intact with a
visible notice. Cleanup failures return an error after preserving the results.

Per-arm immutable request and success checkpoints live beside the run bindings.
Only a confirmed HTTP 204 publishes success; a timeout, refusal or missing
registration retains the credential and reports an unconfirmed outcome. Retrying
uses the same pinned token. A success retry needs no network call and retires
only a still-matching current credential under the stable generation lock. The
run binding permanently archives the retired secret, allowing a fresh adoption
without letting an older retry retire its replacement.

The E2E harness's isolated gateway adopts new slots with `origin: "e2e"` and
`ephemeral: true`, retaining the owner token privately beside the slot under
`.cleanup-owners/`. Observation, adoption and credential publication share the
stable per-slot lock. Existing registrations keep their protection and owner;
a conflicting port refuses attachment. Auto gateway discovery only observes
existing registrations. Generic workspace launchers do not set harness origin.
E2E slots remain reusable rather than being deleted at the end of a suite.

After preserving all measurement and replay inputs, stop the harness's engine
and DELETE `/~/api/v1/control/workspaces/<id>` with JSON
`{"owner_token": "<saved secret>", "confirm": "<current display name>"}`.
The local token alone cannot delete anything. The owner token applies only to
its original registration while that registration remains ephemeral with a
harness origin. Surface refusals and cleanup failures; never broaden the scope
to another registration, a folder sweep, or an unowned database. Manual cleanup
uses paired-device frozen batches; the owner token cannot empty Trash.

Cleanup catalog queries have short statement and lock deadlines.
Recovery dumps and restores instead allow 24 hours, with a 10-second lock timeout
and no per-statement timeout. Large COPY operations can finish within that budget.
A failed transfer retains its journal and recovery resources.
Provisioning and legacy migration do not inherit cleanup subprocess deadlines.

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

## Engine build delivery and Switch

Source and dev installs build into the running engine's published
`.launch/<profile>/<variant>/` directory. The engine passes that directory as
`--publish-to`. The script derives the Cargo profile and feature variant from
it, preserving release and feature builds. An
engine launched outside the published layout reports an actionable failure.
Packaged installs do not run this build flow.

A script exit 0 begins **Verifying**. Only an uncached probe of the published
executable that differs from the running build and covers the current HEAD
makes the build **Ready**. An unchanged or stale executable gets
one automatic fresh retry per HEAD: clean only the engine, gateway and CLI
packages for the selected profile, under the build slot and Cargo's lock,
then build and publish again. A second undelivered result becomes **Failed**
with a cause and **Retry build**. Self-heal stops for that HEAD; explicit Retry
or new source work can start another attempt. Repeated HEAD movement is also
bounded, so an active checkout cannot keep a background build alive forever.

An unavailable executable, HEAD or source-coverage probe reports a verification
failure with Retry; an unknown observation never authorizes a clean build.

Engine and gateway publish failures fail the build; a CLI publish failure
remains non-fatal. The script reports persistent stale publication with exit
75 (`head-moved`). The engine retries for an advanced HEAD, or uses the fresh
retry budget when HEAD is unchanged. Nothing signals Cargo or deletes locks.

Build output is copied into `.lucidos/engine-build.log`. **Show build log**
fetches its last 64 KiB through the workspace-scoped `/api/v1/engine/build-log`
route and shows it inside the paired window, from both progress and failure.
An open viewer refreshes on build-state events. Preserve relevant engine and gateway logs when investigating a missed delivery: gateway log rotation
can remove the build that first exposed it, so current logs alone cannot
establish which publish failed.

A manual source launch builds and generates outputs before stopping the serving
engine. Compile, publish or HEAD-churn failure therefore leaves the current
workspace running. Progress updates from a superseded build cannot replace
its manual retry's state.

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
