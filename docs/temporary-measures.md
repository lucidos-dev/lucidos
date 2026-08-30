# Temporary measures (the impermanence registry)

The single registry for **things in the codebase that are meant to end and carry a
concrete condition for when.** A temporary measure is anything we added knowing
we'd remove it — a workaround until upstream fixes X, a diagnostic that only
exists to chase a bug, a crutch for a model mistake, a feature flag awaiting
cleanup, a back-compat shim with a real removal trigger. Left untracked, every one
of them silently becomes permanent. This file is where they live so removal stays
an intention with an owner and a trigger, not a someday.

This generalizes the old single-category `model-tolerance-measures.md` (now folded
in as one typed section). Every measure goes here, with a removal condition, the
moment it lands — see [`.claude/rules/temporary-measures.md`](../.claude/rules/temporary-measures.md).

## The inclusion test

For every candidate, ask one line:

> **Is this meant to go away, and is there a concrete condition for when?**

**Yes → it belongs here.** No → it belongs elsewhere (see OUT, below).

## Explicitly OUT (so this registry doesn't become a tech-debt swamp)

These are *not* temporary measures — don't add them here:

- **Permanent back-compat / old-data tolerance** — serde aliases for old events,
  parsers that accept a legacy wire shape, defaults for missing historical fields.
  Real requirements that never leave. (Site-local comment is enough.)
- **Site-local suppressions** — `#[allow(...)]`, `@ts-expect-error`,
  `// eslint-disable`. Harden flags those at the site, not here.
- **Permanent design decisions / accepted non-bugs** — these live in
  [`docs/adr/`](adr/README.md) and [`docs/code-review-priors.md`](code-review-priors.md).
- **General refactor wishlist / "tech debt" with no concrete end condition** —
  that's backlog material / [`docs/plans/`](plans/). If you can't write a concrete
  removal condition, it fails the inclusion test.

## How to read an entry

Every entry — across all four sections — uses the same fields:

| Field | Meaning |
|---|---|
| **Added / Opened** | Date the entry landed. |
| **Lives in** | The file(s) / site that carry the thing. `n/a` for a pure investigation. |
| **Impermanent because** | Why this is meant to end (the model mistake it papers over, the upstream bug it works around, the question it chases) — not a permanent requirement. |
| **Removal / resolution condition** | What concretely has to be true to drop it, and how to verify it's safe. "Eventually" is not a condition. |
| **Status** | `open` / `active` (in place) → `resolved` / `removed` + date. Kept as history — never delete a row. |
| **Investigation** | *(measures only)* the parent-investigation id this measure exists to serve (see § Open investigations). Closing the investigation surfaces every measure now eligible for removal. |

When you remove a measure (or close an investigation), set its status to
`removed` / `resolved`, add the date, and leave the entry as a record — don't
delete the row. Also revert any paired docs/wording the measure softened (the
entry's removal condition should name them).

---

## 1. Temporary measures & workarounds

Diagnostics, scaffolding, and "workaround until upstream fixes X" code.

### Toast placement picker

- **Added:** 2026-08-13
- **Lives in:** six sites, all in `crates/lucidos-app/src/`.
  - `toastPlacement` + `ToastPlacement` + `isToastPlacement` in `store/store.ts`
  - its persistence effect in `store/effects.ts`
  - the placement argument to `toastLayout` in `components/shared/toastColumns.ts`
  - the `data-toast-placement` attribute in `components/shared/Toast.tsx`
  - the shape rules in `styles/components.css`
  - the Placement row on `components/settings/CommunicationSurfacesPage.tsx`,
    and `TOAST_PLACEMENT_OPTIONS` in `communicationSamples.ts`
- **NOT the gallery.** The surface gallery those last two sit on is permanent,
  kept so these surfaces can be iterated on. Only the placement picker is
  temporary, and the gallery outlives it.
- **Impermanent because:** It exists to choose ONE shape for the toast stack.
  Per-pane columns, a centred card and a full-bleed bar were each rejected on
  how they look. So the candidates ship together purely to be compared in the
  running app, via the frontend preview. All but one are dead once a shape is
  picked.
- **Removal / resolution condition:** The user picks a shape. Then delete the
  signal, its guard, the effect, the Placement row, `TOAST_PLACEMENT_OPTIONS`,
  the `data-toast-placement` attribute and the losing shapes' CSS. Drop the
  placement argument from `toastLayout` with them. If a cross-pane shape wins,
  `ToastItem.pane` and the per-pane columns go too, per Phase 4 of
  `docs/plans/2026-08-13-toast-banner-dialog-taxonomy.md`. Verify with a
  tree-wide search for `toastPlacement` and `data-toast-placement`, which must
  return nothing, and with the geometry test that phase adds for the winner.
- **Status:** `active`

### Dead-press probe on the composer's action row

- **Added:** 2026-08-26. **Widened:** 2026-08-27, twice; 2026-08-28; 2026-08-29.
- **Lives in:** `crates/lucidos-app/src/components/chat/deadPressProbe.ts`, its
  install call in `src/main.tsx`, and
  `src/components/chat/__tests__/dead-press-probe.test.ts` and
  `src/components/chat/__tests__/dead-press-probe-ledger.test.ts`. The `PressOutcome`
  pair in `src/utils/tapGesture.ts` is part of it: the probe cannot tell a
  served press from a swallowed one, so each consumer says which it was.
- **Impermanent because:** It chases one bug and produces no feature. On an iOS
  PWA the composer's buttons go dead now and then, wherever the finger presses,
  until the keyboard is dismissed. Six reports so far, each able to say only
  "nothing happened", and five fixes shipped against five different readings
  of that. No emulator reproduces it and the user cannot reproduce it on demand,
  so the app itself has to report the next episode.
- **What it watches, and why that changed.** It first named ONE selector,
  `.send-cancel-morph`. The fourth report was three other faces: Diff, the
  answer Submit, and the lone Cancel. In answer mode the morph is not rendered
  at all, so the probe returned at its first check on every tap. It now resolves
  whichever `.action-btn` in `.prompt-actions-row` the press landed on, and
  names it in the report.
- **The five families it tells apart.** The button left the document under the
  finger, which is Apple's documented cascade rule and settles it outright. The
  press never reached the button, so paint and hit-testing disagree. The system
  cancelled a stationary gesture. The face is not reachable at its own painted
  centre, which the fifth round added. Or the press arrived intact and no path
  took it. Every report carries the viewport numbers, the page scroll offset,
  the `pointer-events` at the point, and the `data-keyboard-active` flag.
- **What the fifth report changed.** The probe stayed silent through an episode.
  Its `touchend` arm read `defaultPrevented` as proof a path had worked, and the
  touch path cancels the default before running the action. The actions behind
  it were fixed instead, so a press that runs and does nothing now speaks for
  itself. One of the probe's own silences went with it: a click landing
  anywhere else no longer settles a press.

  It also claimed the reachability check had stopped needing the finger inside a
  painted rect. That was never true of the shipped code, and the eighth report
  below is what found the claim out.
- **What the sixth report changed: the log, not the toast.** It stayed silent
  again, and four of its own branches explain that. It returned on
  `document.activeElement`, which excluded a keyboard iOS held up after focus
  had moved. Its `touchend` arm still bubbled, so any capture-phase
  `stopPropagation` on `document` skipped it, and the overlay contract's paired
  swallow calls exactly that. A press landing in no face's painted rect returned
  with the coordinates unlogged.

  Its fourth silence was the channel itself: a toast reports only to a reader
  who catches it. All four are gone, and `recordPress` now writes every watched
  press to `engine.log` under `[Client/composer-press]`, carrying no draft text.
- **What the seventh report found, and it is the first that names a cause.** The
  probe logged `Cancel: dead` with the finger still, the node connected at the
  lift, the row unchanged and the keyboard up. So WebKit dispatched the touch to
  Cancel and dropped the click, and Cancel was click-only by decision. The fix
  gives a destructive face a touch path that RULES on the tap gate, in
  [`docs/plans/2026-08-28-cancel-survives-the-ios-keyboard.md`](plans/2026-08-28-cancel-survives-the-ios-keyboard.md).
- **What the eighth report found: nothing at all, and that is the finding.** The
  press left no line of any kind. The probe armed only from a `touchstart` it
  could attribute to the row, so a gesture the page never received was invisible
  by construction.

  Three holes made that possible, and all three are closed. The immune
  reachability check sat behind the gate a hit-test disagreement defeats. A
  second touch inside the 600ms grace window erased the first press's verdict
  outright. And no line carried the row's own box.

  Two verdicts were added with them, and both watch the touch pipeline rather
  than the geometry. `no-lift` is a press that arrived and never finished, and
  `click-no-touch` is a click with no gesture behind it at all. iOS standalone
  PWAs are reported to reach both states while `click` keeps working. The full
  reconstruction, and what the platform reports do and do not support, is in
  [`docs/plans/2026-08-29-the-composer-says-when-send-is-unreachable.md`](plans/2026-08-29-the-composer-says-when-send-is-unreachable.md).
- **Reading an episode.** Grep for `composer-press` in whichever log the
  engine's stdout goes to. Under `web-dev.sh -b` that is the workspace's
  `engine.log`, and for a gateway-launched engine it is
  `~/.lucidos/gateway/gateway.log`. Check both, since the workspace file goes
  quiet rather than missing. Each line names the face, the verdict (`served`,
  `swallowed`, `clicked`, `canceled`, `missed`, `dead`, `no-lift`,
  `click-no-touch` or `unreachable`), the travel, the row and face boxes, the
  viewport block and the `data-keyboard-active` flag.
- **Removal / resolution condition:** An episode arrives carrying a verdict, and
  the fix that verdict points at ships, OR two months pass with no report. The
  eighth episode reopened this: the cause is NOT named, and the probe's job is
  evidence again rather than confirmation. Then delete the module, its install
  call, its two tests, the `PressOutcome` pair and its two callers. Flip this
  row to `removed`. Verify with a tree-wide search for `deadPressProbe` and
  `notePressOutcome`, which must return nothing.
- **Investigation:** none. It is narrow enough to stand alone. The three plans
  behind it are
  [`docs/plans/2026-08-27-the-composer-row-reports-which-face-died.md`](plans/2026-08-27-the-composer-row-reports-which-face-died.md),
  [`docs/plans/2026-08-27-the-composer-sends-the-draft-it-is-showing.md`](plans/2026-08-27-the-composer-sends-the-draft-it-is-showing.md)
  and
  [`docs/plans/2026-08-28-a-swallowed-tap-says-so.md`](plans/2026-08-28-a-swallowed-tap-says-so.md).
- **What the ninth report found: the other half of the composer.** The user
  typed and the characters never appeared, so no button was ever pressed. This
  probe was silent and correct, and the textarea had nothing watching it. The
  dead-keystroke probe below covers that half now.
- **Status:** `active`, and back to gathering evidence rather than confirming a
  repair.
- **Not a workaround.** It changes no behaviour and takes no gesture. Real fixes
  ship beside it, and this only decides what the user is told when they fail.

### Dead-keystroke probe on the composer's textarea

- **Added:** 2026-08-29
- **Lives in:** `crates/lucidos-app/src/components/chat/deadKeystrokeProbe.ts`,
  its install call in `src/main.tsx`, its `reportDraftClobbered` caller in
  `src/components/chat/PromptInput.tsx`, and
  `src/components/chat/__tests__/dead-keystroke-probe.test.ts`.
  `src/components/chat/probeViewport.ts` is shared with the press probe above,
  so it goes with whichever of the two is removed last.
- **Impermanent because:** It is the sibling of the press probe and chases the
  same bug from the other side. Nine reports of a dead composer, and the ninth
  is the first to say the TEXTAREA died rather than the buttons: the box took
  focus, the keyboard came up, the user typed, and nothing appeared. No emulator
  reproduces it and the user cannot reproduce it on demand, so the app has to
  report the next episode itself.
- **The three verdicts, and what each settles.** `input-never-arrived` is a
  `beforeinput` with no `input` behind it, which puts the fault in WebKit rather
  than in our code. That is the question nine reports have not been able to
  answer. `keystroke-lost` is the edit reaching the box and not the store.
  `draft-clobbered` is both of those working and a draft clear wiping the
  result, which the composer now repairs rather than obeying.
- **Reading an episode.** Grep for `composer-typing` in whichever log the
  engine's stdout goes to, the same two candidates the press row names. Each
  line carries the verdict, the lengths involved, and the same viewport block a
  press line does, so the two read side by side.
- **Removal / resolution condition:** An episode arrives carrying a verdict, and
  the fix that verdict points at ships, OR two months pass with no report. Then
  delete the module, its install call, the `reportDraftClobbered` call in
  `PromptInput`, and the test. Verify with a tree-wide search for
  `deadKeystrokeProbe` and `reportDraftClobbered`, which must return nothing.
  `resolveEmptyDraftSync` STAYS: it is a behaviour fix, not a diagnostic.
- **Investigation:** none, same as the press probe. The plan behind it is
  [`docs/plans/2026-08-29-the-composer-never-erases-what-you-typed.md`](plans/2026-08-29-the-composer-never-erases-what-you-typed.md).
- **Status:** `active`
- **Not a workaround.** Every listener is passive and consumes nothing. The
  behaviour fix ships beside it, in `resolveEmptyDraftSync`.

### Recorded mirror-history exceptions

- **Added:** 2026-08-11
- **Lives in:** `RELEASE_MIRROR_HISTORY_EXCEPTIONS` in `scripts/lib/release_tree.sh`
  (itself in `RELEASE_TREE_EXCLUDE_PATHS`, so the list never reaches the mirror).
- **Impermanent because:** It carries one commit,
  `df6ef7ca7d748ae5e340993230315ebb096d8095`, the abandoned v0.26.3 Phase A that
  pushed its stripped commit to the mirror's `main` and then died in notarize
  (notarytool `abortedUpload`), so it was never tagged and never published. It is
  the parent of `76345548`, which carries `v0.26.3`, so it cannot be rewritten
  away without changing a published release's SHA. The list exists to record that
  one anomaly rather than tolerate it silently, and it is meant to stay at one
  entry: a second entry means Phase A dropped another commit on `main`, which is
  a bug to fix at the source, not a row to add here.
- **Removal / resolution condition:** Drop the whole array (and the
  `<exceptions>` argument threaded through
  `release_mirror_history_is_complete`) if the mirror is ever rebuilt from the
  published trees for an unrelated reason, since a rebuild re-derives a chain
  with no untagged commits in it. Until then the entry stays, because the
  anomaly it describes is permanent in the published history. Verify by running
  `scripts/lib/release_tree_test.sh` with the array emptied: every assertion
  outside the two exception blocks must still pass, which is what pins that the
  list narrows the guard by exactly its entries and nothing more.
- **Status:** `active`
- **Not a softening of the guard.** Every entry is re-verified on every run
  against the live remote (still an ancestor of `main`, still untagged), and an
  entry that fails either test fails the release by name. Any untagged commit
  that is *not* recorded still refuses the release exactly as before. The stale
  arm is exercised on real data by the test suite, which points the shipped list
  at a stand-in mirror and asserts it is refused.

### One-time mirror history rebuild script

- **Added:** 2026-08-03 (re-armed and wired to a gate 2026-08-04)
- **Lives in:** `scripts/rebuild-mirror-history.sh`, listed in the `paths:`
  frontmatter of `.claude/rules/build-release.md` and in
  `RELEASE_TREE_EXCLUDE_PATHS`.
- **Impermanent because:** It exists to run once. It rebuilds the public
  mirror's `main` as a linear chain of the 36 published releases, replacing the
  parentless commits `release-to-lucidos.sh` published from v0.7 through v0.20.1.
  Its preconditions are pinned to that exact state (`EXPECTED_TAGS=36`,
  `FIRST_TAG=v0.7`, `main` == the newest tag's commit), so it refuses to run the
  moment a 37th release lands, which forces a human re-review rather than a
  silently longer chain. It remains a repair, not a facility: chaining FUTURE
  releases is now done by the pipeline itself (ADR 0039), which is a change this
  script still does not make.
- **The ordering is enforced, not remembered.** Chaining onto a one-commit `main`
  would only ever produce a two-commit `main`, so this has to land before the
  next release. `release.sh`'s Phase A refuses to release while the mirror's
  `main` carries fewer commits than it has `v*` tags
  (`release_mirror_history_is_complete`), and that refusal names this script.
- **Removal / resolution condition:** The rebuilt history is pushed and confirmed
  on the mirror (`git log --oneline lucidos/main | wc -l` reports the release
  count rather than 1). Then delete the script, its `paths:` entry and its
  `RELEASE_TREE_EXCLUDE_PATHS` entry. The Phase A precondition **stays**: it is
  not part of this measure. It is a permanent invariant that self-maintains once
  true, since every release adds exactly one commit and one tag.
- **Note (2026-08-04):** v0.20.0 and v0.20.1 both cut before the rebuild landed,
  so the mirror published this script at both tags with `release_tree.sh`
  withheld, exactly the case the previous removal condition warned about. Adding
  it to `RELEASE_TREE_EXCLUDE_PATHS` is done; the two published copies can do
  nothing but print their own "runs from the internal checkout" refusal.
- **Status:** open

### Legacy credential-cookie fallback

- **Added:** 2026-08-25
- **Lives in:** `gateway/auth.rs::LEGACY_COOKIE_DEVICE_CREDENTIAL` and the
  fallback arm of `::presented_credential`, plus the `migrating` branch of
  `auth_api.rs::enforce` that re-issues on sight. Paired with the migration
  bullet in ADR 0132 § Consequences.
- **Impermanent because:** ADR 0132 gave each gateway its own cookie name,
  because a cookie is scoped to the host and ignores the port. Every browser
  paired before that holds its credential under the old shared name, and
  refusing it would meet those devices with the pairing screen. The fallback
  reads it once and hands back this gateway's own name on the same response.
  It serves only browsers that predate the split.
- **Removal / resolution condition:** Every device in every gateway's store has
  made one authorized request since the split, so each browser has adopted the
  per-gateway name. `last_seen_at` in the store is the read: a row stamped at
  or after the upgrade has been through the migration. Then:
  - Delete the const and the fallback arm, so `presented_credential` reads one
    name and can drop `PresentedCredential` for a plain `Option<&str>`.
  - Delete the `migrating` branch in `enforce`, leaving the daily beat.
  - Delete the legacy-cookie tests in `auth.rs` and `auth_api.rs`.
  - Drop the fallback sentence from ADR 0132 § Decision.

  A device that never comes back was never going to authorize again anyway.
- **Resolved (2026-08-29):** ADR 0162 removed the special case rather than
  waiting it out. Authorization reads every name in the `lucidos_device*`
  family and takes the first that MATCHES a stored device. Whichever name
  carried the match, the response re-issues under this gateway's own.

  So the pre-split name is still read, as one ordinary candidate. There is no
  fallback arm, no `migrating` branch and no const to delete, and a data-dir
  rename is covered by the same rule. `LEGACY_COOKIE_DEVICE_CREDENTIAL` became
  `DEVICE_COOKIE_STEM`, which every name derives from and which is permanent.
- **Status:** resolved

### Legacy paired-device store seed

- **Added:** 2026-08-25
- **Lives in:** `gateway/auth.rs::legacy_paired_devices_path` and
  `::load_or_seed`, called once from `server.rs::run` before the state is
  built. Paired with the "read-only seed" bullet in ADR 0132 § Consequences.
- **Impermanent because:** ADR 0132 moved the paired-device store from the
  machine-global `~/.lucidos/paired-devices.json` to each gateway's own data
  dir. Moving the path alone would refuse every device paired before the
  upgrade, on every gateway at once. The seed copies the old file the first
  time a gateway finds no store of its own, so nobody is locked out. It exists
  only for installs that predate that move.
- **Removal / resolution condition:** Every supported install has booted a
  gateway at or past the release carrying ADR 0132, so each has written its own
  store. Check the oldest version the front door still offers: an install older
  than the seed is the only one needing it. Then:
  - Delete both functions, or keep `load_or_seed` without its `legacy` parameter.
  - Delete the four seed tests in `auth.rs`.
  - Drop the seed bullet from ADR 0132 § Consequences.
  - Drop the boot log line in `server.rs`.

  Lucidos never deletes the old file itself, which stays the user's to remove.
- **Status:** open

### Legacy attached-event-wait boot sweep

- **Added:** 2026-08-06
- **Lives in:** `engine/event_wait/mod.rs::settle_legacy_attached_event_waits`
  (the query) and `engine/event_wait/dispatcher.rs` (the emit), called once from
  `main.rs` before `refire_unresolved_event_wakes` / `rebuild_event_waits`.
  Paired with `ThreadStatus::parse`'s unknown-value fallback to `Idle`, which
  is what reads a stored `waiting_for_event` as what it now means, and the
  migration `20260806090527_event_wait_status_retired.sql`. That was a named
  match arm until 2026-08-07, when `parse` was reduced to
  `try_parse(s).unwrap_or(Idle)` for the `status` filter; the behavior is
  unchanged and the doc comment on `parse` still explains the value.
- **Impermanent because:** It exists for threads caught mid-wait by ADR 0049,
  which removed the attached shape. Such a thread has an `await_event`
  `ToolCalled` with no `ToolResult`, which is a provider 400 on its very next
  turn, and no code left would close that pair for it. Every wait registered
  after the upgrade pairs its own call at registration, so the query can only
  ever match rows written by an older engine.
- **Removal / resolution condition:** No unpaired `await_event` call remains on
  any non-discarded thread. Verify with the sweep's own query returning zero on
  a real workspace (it logs `Closed N legacy unpaired await_event call(s)`, so a
  boot that logs nothing for a while is the signal). Then delete the sweep, its
  `main.rs` call, `legacy_attached_settle_tool_result`, the `waiting_for_event`
  paragraph in `ThreadStatus::parse`'s doc comment, and the note about it on
  `AWAIT_EVENT` in `llm/tool_names.rs`. Nothing is deleted from `parse` itself:
  the fallback that handles the value is load-bearing for any unknown column
  value, not just this one. The migration stays: it is applied history.
- **Status:** open

### Per-workspace model-cache seed and reclaim

- **Added:** 2026-08-11
- **Lives in:** `crates/lucidos-engine/src/memory/legacy_cache.rs`
  (`seed_shared_cache_from_legacy`, `reclaim_legacy_cache`), called from the two
  ends of the background load in
  `engine/memory/embedder_retry.rs::spawn_embedder_load`.
- **Impermanent because:** They exist only to migrate installs off the
  per-workspace embedding-model cache the gateway used to pin
  (`<workspace>/.lucidos/fastembed`, retired by ADR 0061). The seed moves one
  such copy into the shared cache so the upgrade costs no download; the reclaim
  deletes a copy once the model has demonstrably loaded from elsewhere. On a
  fresh install both are no-ops on the first boot and every boot after it, and
  no code writes that path any more, so the only thing they can ever find is a
  directory an older engine left behind.
- **Removal / resolution condition:** No supported upgrade path can still be
  carrying a per-workspace copy, which in practice means every install has booted
  at least once on a version at or after ADR 0061. The signal is the reclaim's
  log line (`Reclaimed N bytes: removed this workspace's leftover model cache`)
  having stopped appearing in the field. Then delete the module, its two call
  sites, and its `pub mod` line. `dir_bytes` moves to wherever it is still
  needed: the gated `test_downloaded_model_loads_without_a_second_fetch` uses it
  to measure the cache tree, and that test is permanent.
- **Status:** open

### Batch `data/` writes announce once for the caller, not per file

- **Added:** 2026-08-01
- **Lives in:** `ArtifactManager::write_batch_and_commit`
  (`crates/lucidos-engine/src/core/artifacts.rs`), registered as an
  `ExemptWriter` on the `core/artifacts.rs` entry in
  `crates/lucidos-engine/src/core/announced_surfaces.rs`.
- **Impermanent because:** Every other `data/` write takes a
  `WriteAnnouncement` and cannot skip its entity event. The batch writer is the
  one hole: it writes N files and announces nothing, relying on its single
  caller (`git_clone`) to emit one `RepositoryImported` for the whole import.
  That is the right behaviour for a bulk import (an entity event per file would
  flood the timeline and index each file separately), but it is expressed as an
  exemption rather than as a choice the caller states, so a second caller of
  `write_batch_and_commit` would write files that announce nothing at all. That
  is exactly the failure mode the surrounding change exists to remove.
- **Removal / resolution condition:** Give `write_batch_and_commit` a
  `WriteAnnouncement` parameter like `write_and_commit` has, with a batch-shaped
  `Entity` arm (one summary event, or per-file when the batch is small) so the
  caller has to state the choice. Then drop the `ExemptWriter` and confirm
  `core::announced_surfaces::tests::every_reachable_data_writer_announces` still
  passes with it gone.
- **Status:** open

### `commit_data_path` announces at the call site, not in the write path

- **Added:** 2026-08-01
- **Lives in:** `ArtifactManager::commit_data_path(s)`
  (`crates/lucidos-engine/src/core/artifacts.rs`) and its callers in
  `crates/lucidos-engine/src/engine/tools/files.rs`.
- **Impermanent because:** These are commit-only helpers: the caller does its
  own `fs::write` and then asks the manager to stage and commit. The manager
  therefore cannot know whether the file pre-existed, so it cannot decide
  Created-vs-Updated, and the entity emit stays with the caller. That leaves the
  file tools on the old shape (write here, announce there) that every other
  `data/` writer has now left behind.
- **Removal / resolution condition:** Route the file tools' writes through
  `ArtifactManager::write_and_commit` instead of writing themselves and
  committing separately, then make `commit_data_path(s)` private. Verify by
  checking that `engine/tools/files.rs` contains no `SystemEvent::artifact_change`
  or `Artifact*` construction, and that the tripwire still passes.
- **Status:** open

### iOS-PWA liveness diagnostic

- **Added:** 2026-05-18
- **Lives in:** `crates/lucidos-app/src/utils/liveness.ts` (heartbeat +
  startup-classification + uncaught-error breadcrumbs, posted to the engine's
  `/api/v1/internal/client-log` endpoint).
- **Impermanent because:** Pure telemetry to chase a bug — it detects iOS WKWebView
  content-process crashes that don't fire `pagehide`/`beforeunload`, classifies each
  startup (`likely_crash` / `bg_resume` / `reload_clean` / …), and captures
  window-level errors so the next blackout leaves evidence. It compensates for
  nothing in the design; once the cause is found and fixed, the whole module is dead
  weight.
- **Removal / resolution condition:** When the **iOS-PWA blackout investigation**
  (`ios-pwa-blackout`) is closed — i.e. the root cause of the iOS PWA black-screen /
  silent content-process kill is identified and fixed and the engine.log
  `[Client/lifecycle]` / `[Client/error]` breadcrumbs are no longer needed to
  reproduce or confirm it. Verify by confirming no open work relies on the
  `kind:"likely_crash"` log line, then delete the module and its call sites
  (`startLivenessTracking` / `reportStartupKind`).
- **Resolved:** 2026-06-30 — investigation `ios-pwa-blackout` closed (paint-loss
  root cause found + fixed; crash half quiet — 0 `kind:"likely_crash"` across the
  live workspaces over their multi-week log spans). **Code retained, NOT deleted**
  — a deliberate deviation from the "delete the module" step above: the
  `postClientLog` breadcrumb channel is shared infrastructure (6+ non-blackout
  call sites — deeplink dispatch, native-tap drain, hash router, ghost-focus
  clear) and the startup-classification / uncaught-error breadcrumbs are reusable
  client-stability telemetry, so `liveness.ts` is kept as permanent debug tooling.
  No longer a temporary measure (now in the spirit of § Explicitly OUT → permanent).
- **Status:** resolved (2026-06-30)
- **Investigation:** `ios-pwa-blackout`

### Thread-render blank-body probe

- **Added:** 2026-06-28
- **Lives in:** `crates/lucidos-app/src/utils/threadRenderProbe.ts`
  (`reportThreadRenderProbe` / `classifyThreadRender`) + its call sites in
  `crates/lucidos-app/src/components/chat/ThreadView.tsx` (the settle-probe effect
  and the `rebuild_corrupted_thread` breadcrumb in the corruption watchdog). Posts
  to the engine's `/api/v1/internal/client-log` endpoint → engine.log
  `[Client/render]` lines.
- **Impermanent because:** Pure telemetry to chase the "thread summary renders but
  the conversation body is blank, recovers on scroll" report on the iOS PWA. The
  data is confirmed present in the DB for every instance, so the fault is downstream
  of the store, but a compositor PAINT loss is invisible to JS — the probe captures
  the discriminating facts (last-rendered vs. fresh-recomputed exchange counts,
  content-DOM child count + height, `animating`, channel) a fixed delay after a
  thread settles and folds them into a class (`missed-rerender` / `empty-render` /
  `dom-missing` / `content-present`) so the next repro distinguishes a stale render
  from a render gap from paint loss. Compensates for nothing in the design.
  **2026-06-29 finding:** every probe in a full afternoon of iOS-PWA use
  (~24 samples) classified `content-present` — zero render-side classes — so the
  blank is confirmed to be compositor **paint loss**, not a store/render fault.
  That drove the scroll-nudge repaint escalation below; the probe stays until the
  fix is confirmed to keep the blank from surfacing over a usage window.
- **Removal / resolution condition:** When the **iOS-PWA blackout investigation**
  (`ios-pwa-blackout`) is closed — i.e. the blank-body root cause is identified and
  fixed and the `[Client/render] thread_render_probe` breadcrumb is no longer needed
  to confirm it. Verify no open work relies on the `class` field, then delete
  `threadRenderProbe.ts` + its test and remove the settle-probe effect and the
  `rebuild_corrupted_thread` breadcrumb call from `ThreadView.tsx`. (The
  paired repaint hardening — the extended `OPEN_REPAINT_BURST_DELAYS_MS` tail, the
  settle re-burst, and the `forceIOSRepaint` scroll-nudge + forced layout-read
  escalation [`docs/plans/2026-06-29-ios-pwa-blank-thread-scroll-nudge-repaint.md`]
  — is a real fix, NOT telemetry, and stays.)
- **Resolved:** 2026-06-30 — investigation `ios-pwa-blackout` closed: the
  blank-body is confirmed compositor **paint loss** (the 2026-06-29 finding +
  zero render-side problem-class breadcrumbs since), fixed by the repaint
  hardening. **Code retained, NOT deleted** — `threadRenderProbe.ts` is the most
  symptom-specific of the three probes, but it is tiny and silent when idle, so it
  is kept available to confirm a future blank-render regression rather than
  deleted. No longer a temporary measure.
- **Status:** resolved (2026-06-30)
- **Investigation:** `ios-pwa-blackout`

### The render probe's WebKit surface

- **Added:** 2026-08-26
- **Lives in:** `crates/lucidos-app/src/utils/threadRenderProbe.ts`, at
  `reportThreadRenderProbe`'s `isWebKit()` gate. The probe module itself is
  retained tooling (see the row above); this row covers only how WIDE its
  reporting surface is set.
- **Impermanent because:** the gate was `isIOSPwa()`, which held the breadcrumb
  to the one surface the blank was reported on. The blank came back on the
  packaged Mac app, whose Tauri window is a WKWebView. So the surface widened to
  every WebKit client, to catch it there too. The price is one engine.log line
  per thread open on such a client, which buys a diagnosis and nothing else.
- **Removal / resolution condition:** when `webkit-desktop-blank-thread` closes,
  narrow the gate back to `isIOSPwa()`. Verify by confirming no open work reads
  a `[Client/render] thread_render_probe` line from a desktop client.
- **Status:** open
- **Investigation:** `webkit-desktop-blank-thread`

### Click-lag main-thread blocker probe

- **Added:** 2026-06-24 (registered 2026-06-29 by the nightly harden sweep — the
  probe predated this row)
- **Lives in:** `crates/lucidos-app/src/utils/perfProbe.ts` (`startPerfProbe`) +
  its call site in `crates/lucidos-app/src/main.tsx` (`startPerfProbe()`, guarded
  by `!IS_PICKER`, with the `TEMP:` comment). Logs `[perf-probe]` lines to the
  browser console only (no engine post).
- **Impermanent because:** Pure diagnostic chasing the "button/drawer clicks slow
  to register" lag in the dev workspace. It wires three `PerformanceObserver`s
  (Event Timing, Long Animation Frames, long tasks) that stay quiet below their
  thresholds and, on a slow interaction, split the cost into input-delay / handler
  / render and name the script behind a long frame — the discriminating "JS
  re-render vs DOM layout vs paint" signal. Compensates for nothing in the design;
  its own header says "REMOVE once the cause is found."
- **Removal / resolution condition:** When the **click-lag investigation**
  (`click-lag`) is closed — the main-thread blocker behind the laggy clicks is
  identified and fixed and the `[perf-probe]` console lines are no longer needed to
  reproduce or confirm it. Verify no open work relies on the `[perf-probe]` output,
  then delete `perfProbe.ts` and remove the `startPerfProbe()` call + import +
  `TEMP:` comment from `main.tsx`.
- **Resolved:** 2026-06-30 — investigation `click-lag` retired: the lag is no
  longer perceived (root cause never pinned, but not actively chased). **Code
  retained, NOT deleted** — `perfProbe.ts` is fully general (Event Timing / Long
  Animation Frames / long tasks name the script behind *any* slow interaction,
  nothing click-lag-specific), so it is kept as permanent perf instrumentation —
  registered always but silent below its thresholds, so it surfaces a future
  interaction-lag regression without reactivation. No longer a temporary measure.
- **Status:** resolved (2026-06-30)
- **Investigation:** `click-lag`

### CC "No conversation found" error-string match (stale-resume detection)

- **Added:** 2026-07-01
- **Lives in:** `crates/lucidos-engine/src/engine/agent_session/lifecycle.rs`
  (`is_definitive_session_not_found` — substring match on `"No conversation found
  with session ID"`), consumed by the stale-resume branch in
  `crates/lucidos-engine/src/engine/agent_session/run_session/run.rs`.
- **Impermanent because:** matches Claude Code's **human-readable** error prose to
  detect a definitively-gone resume session, because the `result` event we parse
  (see `runtime/claude_code_parse.rs`) exposes no structured "session not found"
  code/subtype — only free-text `errors`. A prose match is fragile across CC
  versions; we'd prefer a structured signal. (Distinct from the empty-echo
  heuristic `is_stale_resume_signal`, which needs no upstream change.)
- **Removal / resolution condition:** when Claude Code's `result` event surfaces a
  structured session-not-found signal (a stable `subtype`/error code), switch
  `is_definitive_session_not_found` to match on that field instead of the prose,
  and drop the substring test. Verify by confirming the new field appears on a real
  `--resume <bogus-id>` result event, then update the predicate + its unit tests in
  `lifecycle_tests/classify.rs`.
- **Status:** active
- **Investigation:** n/a (root cause is a known engine gap, fixed in
  `docs/plans/2026-07-01-cc-resume-config-dir-pin-and-session-not-found.md`; this row
  tracks only the string-match fragility)

### "not published yet" hedge on the front-door uninstall one-liner

- **Added:** 2026-07-30
- **Lives in:** the usage comment at the top of `uninstall.sh` (the
  `curl -fsSL https://lucidos.dev/uninstall.sh | sh` line, now annotated
  `NOT PUBLISHED YET` with the explanation under it), and the paragraph in
  `README.md` § "Manage / uninstall" that routes a one-liner installer to the
  repository copy instead of to a front-door one-liner.
- **Impermanent because:** the site publisher uploads `install.sh` and
  `scripts/lib/*.sh` beside it, but **not** `uninstall.sh`, so
  `https://lucidos.dev/uninstall.sh` returns the Cloudflare Pages landing page at
  status **200**. The one-liner is the intended, correct front door; the docs
  advertising it are right about the destination and wrong only about *today*. So
  the hedge annotates the line rather than replacing the URL with a raw
  `githubusercontent` one, which would undo commit b0421c862 and have to be
  reverted the moment the publisher catches up.
- **Removal / resolution condition:** when
  `curl -fsSL https://lucidos.dev/uninstall.sh | head -c 2` prints `#!` rather than
  `<!`. Then drop the annotation from `uninstall.sh` and give the README paragraph
  the front-door one-liner instead of the repository link. The shebang sniffs added alongside it
  in `install.sh` (`dispatch_uninstall`, the re-exec guard) and `uninstall.sh` are
  **permanent** defence in depth, exactly like `_source_libs`, and do NOT come out
  with this row.
- **Status:** active
- **Investigation:** n/a (publisher gap, recorded in
  `docs/plans/2026-07-30-user-facing-docs-audit.md` and previously deferred in
  `docs/plans/2026-07-29-front-door-origin-and-rc-gate.md`)

### Cross-document notification-tap reload on iOS

- **Added:** 2026-07-31 (the cross-document navigate URL itself landed 2026-05-31,
  §4.5 seventh iteration; this row registers it and its new page-side cover)
- **Lives in:** `crates/lucidos-engine/src/scheduler/push.rs` (`navigate_url_ios`,
  the query-string URL put on `notification.navigate`) and, page-side, the
  **deep-link branch** of the `boot-splash-quiet` script in
  `crates/lucidos-app/index.html` (the `?notification=` detection, NOT the cover
  it turns on).
- **Scope note (2026-07-31, same day):** the *quiet boot cover* itself is
  **permanent** and is NOT part of this measure. It has a second trigger with no
  end date: a user-requested refresh (`refreshClient` stamps
  `lucidos-splash-quiet`), which is a continuation of the session for exactly the
  same reason a tap is. So this row tracks only the notification-tap TRIGGER and
  the engine-side URL that forces it. The cover, its stylesheet rules, and the
  `boot-splash-quiet` arm of `bootSplashPlaysNoReveal()` all outlive it.
- **Impermanent because:** A push tap should hand the deep link to the PWA window
  that is already open. Instead the engine deliberately emits a **cross-document**
  query URL, so every tap tears the running document down and boots a new one.
  That is not a design preference: it is the only channel WebKit applies. Safari
  implements neither `launchQueue` nor `launch_handler: focus-existing`, and it
  will not apply a same-document (hash) navigate to an open window (it just
  focuses it, and the deep link silently no-ops, which is the bug the seventh
  iteration fixed). So the reload is a workaround for an upstream gap, and the
  page-side detection that recognizes such a load exists only to serve it: once a
  tap no longer reloads, there is no notification-tap document left to recognize.
  Measured cost of the gap on the reporting iPhone: a full document boot, 335-1132
  ms of bundle load before any app code runs, on every single notification the
  user opens.
- **Removal / resolution condition:** When WebKit ships a reload-free channel for
  a push tap into an open window, i.e. **either** `launch_handler:
  focus-existing` + `window.launchQueue` (feature-detect `'launchQueue' in
  window` on a real installed PWA, not in a desktop simulator) **or** a declarative
  `notification.navigate` that Safari applies same-document to an already-open
  window. Verify on-device that a tap routes the deep link with no new document
  (`[Client/lifecycle] startup` emits nothing for the tap, and
  `[Client/deeplink] handle_hash_location` still reports `found:true`). Then
  switch `navigate_url_ios` to the reload-free form and delete **only the
  deep-link trigger**: the `if (!quiet) { … }` URL-detection block in the inline
  script (leaving the `lucidos-splash-quiet` flag read that precedes it), the
  deep-link cases in the `notification tap (boot-splash-quiet)` suite in
  `bootSplash.test.ts`, and the §4.5 wording that documents the reload as
  unavoidable. **Keep everything else**, per the scope note above: the cover, its
  stylesheet rules (including the light-theme foreground and reduced-motion
  restatements), the refresh trigger, and `bootSplashPlaysNoReveal()` in both its
  arms. Deleting the cover with the trigger would silently restore the cold-launch
  animation on every refresh, which no upstream fix has anything to do with.
- **Status:** active
- **Investigation:** n/a (the cause is known and upstream; nothing is being
  chased here, only waited on)

### `x-safari-` scheme prefix to escape an iOS standalone PWA

- **Added:** 2026-08-02
- **Lives in:** `crates/lucidos-app/src/utils/openExternalUrl.ts`
  (`SAFARI_SCHEME_PREFIX` and `handOffToSafari`, reached by the `safari` mode and
  as the fallback from `ask`). Every external-link surface arrives through
  `openUrl` (`store/actions/artifacts.ts`), including app iframes via
  `lucidos.ui.openExternal`, so this one site is the whole measure.
- **Scope note (2026-08-02):** the `external_link_target` preference added the
  same day is **permanent** and is NOT part of this measure. `ask` (the OS share
  sheet) and `in-app` are ordinary platform behaviours that need no undocumented
  scheme; only the `safari` mode's `x-safari-` prefix is the workaround. If the
  prefix goes, the preference keeps its three modes and `safari` simply becomes
  a plain `window.open`.
- **Impermanent because:** Inside an installed iOS PWA (`display-mode:
  standalone`) WebKit refuses to hand a URL to the Safari app: both
  `window.open(url, '_blank')` and `<a target="_blank">` render in the PWA's own
  in-app web view, which has no address bar, no tabs, no shared Safari session,
  and no way back to a real browser. `x-safari-https://…` is an **undocumented
  Apple URL scheme**, not a web standard, and it is the only channel that
  works. So this is a workaround for an upstream gap, on the same footing as the
  cross-document notification-tap reload above: the day WebKit honours the
  standard affordance, the prefix is pure liability, and being undocumented it
  can be withdrawn without a deprecation.
- **Removal / resolution condition:** When an installed iOS PWA opens
  `window.open(url, '_blank', 'noopener')` in the **Safari app** rather than the
  in-app web view. Verify on a real device (not the desktop responsive simulator
  or Playwright's `mobile-webkit` project, where `isIOSPwa()` is false and the
  branch never runs): install the PWA to the home screen, tap an external link in
  a chat response, and confirm the address bar and tab bar are present and the
  page shares the Safari session. Then delete `SAFARI_SCHEME_PREFIX` and
  `handOffToSafari`, point the `safari` mode and the `ask` fallback at
  `window.open`, and drop the `x-safari-` cases from `openExternalUrl.test.ts`
  and `artifacts-open-url.test.ts`. **Keep everything else**, per the scope note
  above: the `external_link_target` preference and all three of its modes, the
  Settings row, `lucidos.ui.openExternal` plus its cache, and `openExternalUrl`
  itself even once it collapses to a lone `window.open`. It is the single choke
  point every external-link surface routes through, and the delegation guard in
  `useStartup.test.ts` is pinned to that funnel for reasons that outlive this
  measure.
- **Status:** active
- **Investigation:** n/a (the cause is known and upstream; nothing is being
  chased here, only waited on)

### CC byte-idle deadline raised past the engine watchdog

- **Added:** 2026-08-02
- **Lives in:** `CC_BYTE_STREAM_IDLE_TIMEOUT_MS` and the
  `CLAUDE_BYTE_STREAM_IDLE_TIMEOUT_MS` env write in `build_command`
  (`crates/lucidos-engine/src/runtime/claude_code.rs`); the ordering invariant is
  asserted against `agent_session::lifecycle::WATCHDOG_INACTIVITY_LIMIT_MS` in
  `runtime/claude_code_tests/build_command.rs`.
- **Impermanent because:** it works around an upstream client default. Claude Code
  aborts a turn after 300 000 ms of zero bytes on the SSE body and reports
  `API Error: Stream idle timeout - no chunks received`, terminally: no
  non-streaming fallback, and at most one retry that is unavailable once
  `message_start` has arrived. At the 200k+ token contexts a coding-agent session
  reaches, a cache-cold prompt is silent on the wire for longer than that
  (measured: two deaths at 303 s on one thread, 2026-08-02). We raise the deadline
  to CC's clamp maximum purely so the engine's own 10-minute inactivity watchdog,
  whose response is a non-destructive kill plus auto-resume, becomes the first
  responder. If CC either retried this error class itself or defaulted above our
  watchdog, we would set nothing.
- **Inert since at least `2.1.224` (found 2026-08-10):** CC has two idle tiers and
  the effective deadline is the lower of them. The env write moves the **byte**
  tier; the **event** tier is `max(CLAUDE_STREAM_IDLE_TIMEOUT_MS || 0, 300000)`,
  arms its own abort, and is the one that actually fires. So the real deadline is
  still 300 s and the intended ordering was never achieved (measured: a death at
  304.9 s with `CLAUDE_BYTE_STREAM_IDLE_TIMEOUT_MS=1800000` confirmed on the live
  subprocess). The test in `build_command.rs` asserts the ordering against the
  *constant*, so it passes while the invariant is false. Left as-is deliberately,
  because `auto_resume_after_api_error` (shipped 2026-08-05) now catches this
  error class three seconds later and makes the ordering close to moot. Fixing it
  means also writing `CLAUDE_STREAM_IDLE_TIMEOUT_MS` and widening that test to
  both tiers.
- **Removal / resolution condition:** drop the env write when EITHER (a) Claude
  Code auto-resumes a stream-idle failure rather than ending the turn, verified by
  seeing a session recover from the error with no `ResponseFailed`, or (b) its
  default byte-idle deadline exceeds `WATCHDOG_INACTIVITY_LIMIT_MS`, verified by
  re-reading the resolver in the installed bundle. Note that neither is satisfied
  by the engine's own `auto_resume_after_api_error`: that recovers the thread but
  says nothing about CC's deadline, and it is a Lucidos behaviour rather than the
  upstream change both arms are waiting on. On removal, delete
  `CC_BYTE_STREAM_IDLE_TIMEOUT_MS`, both tests in
  `runtime/claude_code_tests/build_command.rs`, and the `pub(crate)` justification
  paragraph added to `WATCHDOG_INACTIVITY_LIMIT_MS`.
- **Status:** active
- **Investigation:** n/a (cause fully characterized in
  `docs/investigations/2026-08-02-cc-stream-idle-timeout.md`; the upstream reports
  are closed as duplicate / not planned, so there is nothing open to chase)

### "list_files returns the whole tree unfiltered" line in `[CURRENT FILES]`

- **Added:** 2026-08-07
- **Lives in:** `build_file_list_section`
  (`crates/lucidos-engine/src/engine/chat/process/workspace_payload.rs`), the
  trailing advisory emitted whenever the workspace holds a vendored file. The
  block itself is never partial: it is an inventory or nothing (ADR 0086 as
  amended), and vendored paths are the one thing it names by count instead.
- **Impermanent because:** it warns the chat agent about a defect in a
  neighbouring surface rather than describing its own. The `list_files` tool
  (`engine/tools/files.rs`) returns `all_files.join("\n")` with no ignore filter
  and no cap, so on a workspace with a vendored tree one call returns every
  path, which is exactly what the prompt block was reshaped to stop paying for.
  Warning the model is a mitigation; filtering the tool is the fix.
- **Removal / resolution condition:** when `list_files` applies
  `core::artifacts::is_vendored_path` (or otherwise bounds its result), reword
  the line to drop the "returns the whole tree unfiltered" claim, which is
  otherwise a false statement the prompt makes on every turn. Keeping a shorter
  "use glob_files for a targeted lookup" pointer is fine. Verify by checking
  that the `list_files` arm in `engine/tools/files.rs` filters, and that
  `vendored_tree_is_excluded_and_real_files_survive` still asserts whatever the
  line becomes.
- **Status:** active

### macOS function-key characters inserted as text at a caret boundary

- **Added:** 2026-08-15
- **Lives in:** `crates/lucidos-app/src/utils/noFunctionKeyText.ts` and its
  `installNoFunctionKeyText()` call in `crates/lucidos-app/src/main.tsx`.
- **Scope note:** the native half, `install_app_menu`
  (`crates/lucidos-app/src/lib.rs`), is **permanent** and is NOT part of this
  measure. A complete app menu is what loads the standard text-editing key
  bindings, so without it no arrow key moves the caret at all. This row covers
  only the frontend guard.
- **Impermanent because:** AppKit reserves 0xF700 to 0xF8FF for function keys,
  so the right arrow's key event carries 0xF703 as its characters. macOS maps
  the key to `moveRight:`. At the end of the text that command has nothing to
  move over, so the keystroke falls through to plain text insertion. WebKit's
  guard there rejects only control characters below 0x20, so the private-use
  character lands in the field as a tofu square. The user reported it against
  the chat prompt in the desktop app. No web page should receive such a
  character as text, so the guard compensates for the embedded webview rather
  than our own design.
- **The refusal stops at 0xF747, not 0xF8FF, and that bound replaces a platform
  gate.** Apple assigns constants only up to Mode Switch. No key event can carry
  anything above it, and the rest of the private-use block belongs to the fonts
  that squat there. A narrow unconditional guard beats a wide one gated on
  Tauri. The gate would still refuse a Character Viewer glyph in the desktop
  app, the one client where a user is likeliest to insert one.
- **Not covered: app iframes.** A document-level listener cannot reach into an
  app iframe, so an app's own text field still takes the character. Nobody has
  reported it there, and the fix would be the same guard in the SDK.
- **Removal / resolution condition:** when a right arrow at the end of a prompt
  inserts nothing in the packaged desktop app, with the listener disabled. Check
  the other three arrows and Page Up/Down too, since each takes the same path at
  its own boundary. Then delete the module, its test, and the call plus import
  in `main.tsx`. Drop the paragraph the guard added to `install_app_menu`'s
  docstring, and keep the menu itself.
- **Status:** active
- **Investigation:** n/a (the cause is upstream in the embedded webview, and
  nothing is being chased here)

### Prompt-cache wire probe

- **Added:** 2026-08-17
- **Lives in:** `crates/lucidos-engine/src/llm/cache_probe.rs` plus
  `cache_probe_tests.rs`, gated on `LUCIDOS_CACHE_PROBE`. Everything outside
  that module is thin, and all of it is in this list.
  - `pub(crate) mod cache_probe;` in `crates/lucidos-engine/src/llm/mod.rs`.
  - `log_request` and `log_response` in
    `crates/lucidos-engine/src/llm/anthropic_wire.rs`, at the end of
    `build_claude_request` and `parse_claude_stream`.
  - the `url` field on both `WireTarget` variants in the same file, plus the
    `'a` lifetime it forced onto the enum. With it go the `request_url`
    binding in `build_claude_request`'s target match and the two construction
    sites (`llm/vertex/claude.rs`, `llm/anthropic/chat.rs`). In
    `anthropic/chat.rs` the `messages_url` binding also moved above the
    builder call to feed it.
  - `VERTEX_TEST_URL` and `the_probe_url_argument_never_reaches_the_body` in
    `anthropic_wire.rs`'s test module. Three other tests there construct a
    `WireTarget` and need the field dropped, not deleting.
  - the `scope` wrapper around `provider.chat` in
    `crates/lucidos-engine/src/engine/agentic_loop/run.rs`.

  Writes to engine.log only: no event, no DB row, no UI.
- **Impermanent because:** Pure telemetry for the **first-of-turn prompt-cache
  miss** investigation below. `ContextCaptured` records section composition, not
  the serialized bytes, which is why 578k rows could rule out every candidate
  and still not name the cause. The probe hashes each cache-prefix segment on the
  exact JSON that goes on the wire, so two consecutive calls diff mechanically.
  It compensates for nothing in the design and changes no behaviour.
- **Removal / resolution condition:** When the **first-of-turn prompt-cache
  miss** investigation (`prompt-cache-first-of-turn-miss`) is closed, that is,
  the cause is identified and either fixed or attributed upstream. Verify no open
  work still needs the `[CacheProbe]` lines. Then delete every site in the
  "Lives in" list above, which is written to be followed top to bottom. The env
  var then has no reader, so remove its bullet from the `lucidos-env-vars` skill
  in the same change. `grep -rE 'CacheProbe|LUCIDOS_CACHE_PROBE|cache_probe' .`
  must come back empty, and `make lint` must pass: the `WireTarget` lifetime is
  the one piece a partial removal leaves behind compiling.
- **Status:** active
- **Investigation:** `prompt-cache-first-of-turn-miss`

### Movement distance in the discarded-tap toast

- **Added:** 2026-08-24
- **Lives in:** `crates/lucidos-app/src/components/chat/PromptInput.tsx`
  (`morphTapPassed`), which reads `tapRejection()` from
  `crates/lucidos-app/src/utils/tapGesture.ts` and puts the number in the
  toast. The toast is permanent; only the `moved Npx` half is not.
- **Impermanent because:** The scroll-vs-tap gate silently ate real taps on the
  composer's Submit with the iOS keyboard up. The cause was the gate measuring
  page-viewport coordinates instead of finger movement, so the fix switched it
  to screen coordinates. The bug is intermittent, so normal use is the only way
  to confirm the fix landed. A bare "Tap ignored" cannot tell a fixed gate from
  a still-broken one, and the number can. A rejection at 30px is a real swipe,
  where one at 9px is the gate still measuring something that is not the finger.
- **Removal / resolution condition:** When the user confirms the composer's
  Submit no longer misfires on an iOS PWA with the keyboard up. A reported
  rejection whose distance names a remaining cause closes it too. Then drop the
  distance from the message in `morphTapPassed`, keeping the plain sentence.
  Drop `tapRejection` from the gate and its tests in `tapGesture.test.ts` if
  nothing else reads it. The toast itself stays: a
  discarded press is user intent dropped, which the no-hidden-errors rule
  requires surfacing.
- **Status:** active

### Native cursor mirroring

- **Added:** 2026-08-25
- **Lives in:** nothing now. It occupied five sites, all under
  `crates/lucidos-app/`, and all five are deleted:
  - `src/cursor.rs`, the keyword table and the `set_window_cursor` command
  - that command's entries in `permissions/app-ipc.json` and `generate_handler!`
  - `src/utils/nativeCursor.ts`, the reconciler, and its two test files
  - the `installNativeCursor()` call in `src/main.tsx`
  - the `setWindowCursor` wrapper in `src/utils/tauri.ts`
- **Impermanent because (the claim at the time, since refuted):** ADR 0129 held
  that `tao` lays an arrow cursor rect over its whole content view. It held that
  AppKit re-asserts that rect as the mouse moves, against WebKit writing the
  cursor from CSS on the same moves. Two writers, one cursor. **None of that was
  true**, and ADR 0134 refutes each part.
- **Removal / resolution condition (as recorded, never reached):** when the
  `tao` we ship let WebKit own the cursor. It was moot from the start, because
  tao never took the cursor in the first place.
- **Removed:** 2026-08-25. The mechanism was inert, not merely unnecessary: wry
  evicts tao's content view, so the invalidate it triggers lands on a view
  AppKit holds no rects for. All five sites are deleted. ADR 0134 records the
  evidence, the real mechanism (WebKit declines under four native guards), and
  the open question.
- **Confirmed after removal:** a machine restart cleared the symptom in the
  v0.30.4 bundle, which still carried the mirroring. Docker Desktop showed the
  identical symptom over the same period. The cause is machine-wide, so do not
  restore this mechanism if the arrow returns. ADR 0134 lists the three checks
  to run instead.
- **Status:** removed (2026-08-25)

---

## 2. Model-tolerance measures

A **model-tolerance measure** is a crutch added *only* to compensate for current
LLMs making a predictable, recurring mistake — forgiving where we'd otherwise be
strict, so a weak model's wrong-but-intuitive guess still works. The honest reason
it exists is "the model keeps getting this wrong," not "the design needs it." As
models improve and stop making the mistake, each becomes dead weight to remove.
(This is *not* for genuine ergonomics, backward-compat, or external-robustness —
those stay. The test: *would we still want this if the model were perfect?* No →
it's a tolerance measure.) For these entries, **Impermanent because** names the
exact model mistake the measure tolerates.

### "The model" means every model in the registry, not the newest one

Every removal condition below is phrased as "when **the model** reliably stops
doing X." That singular was written when one Anthropic family served every
request. It no longer holds: the *model registry* routes chat across Anthropic
(Vertex + direct), OpenAI, OpenRouter (Kimi K3, GLM 5.2), and a configurable
local OpenAI-compatible endpoint, and any enabled model can be a user's
`chat_model`.

So read every "the model" below as **the weakest model the registry routes to**,
not the newest one. A crutch is dead weight only once every routed model has
stopped needing it. Two consequences:

- **A new flagship is not evidence of removability.** Adding a stronger default
  doesn't retire a crutch that a weaker routed model still trips over. Sample
  across the models actually in use, not just the current default.
- **Enabling a model can make a measure MORE load-bearing.** A tolerance keyed
  to one family's failure shape (e.g. the Anthropic `<invoke name=…>` tool-call
  serialization) doesn't cover the shapes other families leak, so adding a
  non-Anthropic model widens what the crutch has to survive rather than
  narrowing it.

**Sample honestly.** A grep that returns zero is only evidence if the sampled
population actually exercised the choice the measure tolerates. Count the
denominator — occurrences where the model *made* the decision — not the raw file
or turn count, and say what the denominator was. A removal condition that can be
satisfied by a population that never made the choice is a badly-written
condition; fix the condition rather than acting on it.

### CC `is_error: true` + `subtype: "success"` contradiction (no fabricated failure)

- **Added:** 2026-07-02
- **Lives in:** `crates/lucidos-engine/src/runtime/claude_code_parse.rs` (the
  `"result"` arm's error derivation), covered by
  `parse_result_is_error_with_success_subtype_yields_no_error`,
  `parse_result_is_error_with_success_subtype_and_text_yields_no_error`,
  `parse_result_is_error_success_subtype_preserves_api_error_result_text`,
  `parse_result_is_error_success_subtype_ignores_incidental_api_error_mention`,
  and `parse_result_is_error_with_no_subtype_and_no_errors_yields_no_error` in
  `crates/lucidos-engine/src/runtime/claude_code_tests/parsing.rs`.
- **Impermanent because (tolerates):** Claude Code sometimes stamps its final
  `result` event with `is_error: true` while *also* labelling the turn
  structurally successful (`subtype: "success"`, or an absent subtype) and
  omitting `errors[]` — a self-contradictory terminal signal. Treating that as a
  failure fabricated a generic `"Unknown error"` `ResponseFailed` on turns that
  had streamed a full response and committed work (the red "Event stream error /
  Unknown error" on the OPUS Brand Title Badge Updates thread, and a second thread
  in the same log window; it also tripped `ResponseFailed`-subscribed triggers).
  The parser now returns `error: None` when `is_error: true` carries no actionable
  detail (no `errors[]` and a subtype that is empty or `"success"`), deferring the
  terminal decision to `classify_result` — EXCEPT when the result text is CC's own
  `API Error: …` message (a genuine upstream drop CC still labelled successful),
  which is preserved as the real failure reason (matched on a leading `API Error`
  prefix, never a loose substring, so an incidental mid-sentence mention isn't
  mis-flagged). We would not need this if CC never emitted `is_error: true` on a
  turn it declared successful.
- **Removal / resolution condition:** When Claude Code stops emitting
  `is_error: true` alongside `subtype: "success"`/absent with an empty `errors[]`
  (its terminal signal becomes self-consistent) — sample a batch of recent CC
  `result` events; if none pair `is_error: true` with the success/empty subtype
  and no `errors[]`, drop the `None` branch + the `API Error` prefix carve-out
  (restore the plain subtype fallback) and its tests. Informative subtypes
  (`error_max_turns`,
  `error_during_execution`, …) and `errors[]`-bearing results are unaffected by
  this measure and stay classified as `Failed`.
- **Status:** active

### CC `API Error` prefix as the transient-vs-permanent failure signal (auto-resume)

- **Added:** 2026-08-05
- **Lives in:** `crates/lucidos-engine/src/engine/agent_session/lifecycle.rs`
  (`is_transient_api_failure`, read by `auto_resume_after_api_error`), covered by
  `transient_api_failures_are_recognized_by_the_api_error_prefix` and
  `deterministic_failures_are_not_transient` in
  `crates/lucidos-engine/src/engine/agent_session/lifecycle_tests/watchdog.rs`.
- **Impermanent because (tolerates):** Claude Code's `result` event exposes no
  structured code for "the upstream connection died mid-response" as distinct
  from "this turn is over and would fail the same way again". The only signal is
  the human-readable text CC streams as the turn's final message, whose one
  stable feature is a leading `API Error` (`API Error: Connection closed
  mid-response.`, `API Error: Stream idle timeout - no chunks received`, `API
  Error: 529 overloaded`). The engine keys the bounded auto-resume on that
  prefix, so a wording change upstream would silently stop recovering the exact
  failure this exists for. Same string contract as the `is_error: true` +
  `subtype: "success"` row above, which is why both match the PREFIX rather than
  a loose substring: they must agree on what the string means.
- **Removal / resolution condition:** When Claude Code's `result` event carries a
  structured transient/retryable signal (an error code, a `retryable` flag, a
  stable `subtype` for a stream drop), switch `is_transient_api_failure` to read
  it and keep the prefix only as a fallback for old CC versions, then drop the
  fallback once the minimum supported CC version emits the structured field.
  Verify by sampling recent CC `result` events with `is_error: true` and checking
  every one carries the structured signal.
- **Status:** active

### CSS font-variable aliases (`--font-family`, `--font` → `--font-ui`)

- **Added:** 2026-06-25
- **Lives in:** `crates/lucidos-engine/src/api/sdk_iframe.css` (`:root` token block,
  served to every app iframe at `/api/v1/sdk-iframe.css`).
- **Impermanent because (tolerates):** Coding agents building apps reach for
  `var(--font-family)` (it mirrors the CSS *property* name) or the `--font`
  shorthand instead of the canonical `--font-ui`. An undefined `var()` silently
  falls back to the app's hardcoded stack, so the app shipped the system font
  instead of the user's chosen font — with no error. `building-an-app.md` already
  named `--font-family` as a mistake to avoid and it kept recurring, so guidance
  alone was insufficient.
- **Removal / resolution condition:** When app coding-agent threads reliably emit
  `--font-ui`. Sample only apps **created after 2026-06-25** (this measure's Added
  date — an older app can't evidence a post-measure habit) and classify each as
  *token-choosing* if any of its `index.html` / `*.css` references `--font-ui`
  **or** an alias (`--font-family` / `--font`). **The denominator is the
  token-choosing apps only.** Apps that name no UI-font token are silent for a
  reason that says nothing about naming: `sdk-iframe.css` already sets
  `body { font-family: var(--font-ui) }`, so an app never has to name the token to
  get the user's font, and most don't. Counting those apps makes a clean grep look
  like a cured habit when it is really an empty sample. A `--font-mono`-only app is
  likewise not token-choosing — `--font-mono` is a separate token with no
  competing mis-guess. Drop the aliases when **at least 10 token-choosing apps**
  created after 2026-06-25 use `--font-ui` and **none** use an alias. Fewer than 10
  is an insufficient sample, **not** a pass — in particular a sweep that finds
  *zero* token-choosing apps is no evidence at all; leave the measure active and
  re-sweep later. On removal, also drop the "tolerated aliases" notes in
  `system-knowhow/js-sdk.md` § Theme variables and
  `system-knowhow/building-an-app.md`, and restore the stricter "don't invent
  `--font-family`" wording.
- **Status:** active — last swept 2026-07-25 across two workspaces: of the apps
  created after 2026-06-25, only 3 were token-choosing (all `--font-ui`, no
  aliases). Below the 10-app threshold, so the measure stays. The one alias use in
  the sampled period predates the measure and is not a post-measure regression.

### Gemini's plan narration is kept off the screen

- **Added:** 2026-08-20
- **Lives in:** `crates/lucidos-engine/src/llm/vertex/gemini.rs`
  (`build_gemini_llm_response`, the `narration` branch).
- **Impermanent because (tolerates):** Gemini writes its reasoning into
  ordinary, non-`thought` text parts beside a `functionCall`, even with
  `includeThoughts: true` asking for it in `thought` parts instead. Printing it
  would show the user a monologue rather than an answer. Measured on
  `gemini-3.5-flash`, 10 of 15 samples on one round emitted such a part;
  `gemini-3.1-pro-preview` emitted one in 16. So `content` is held at `None` for
  that turn, and the text rides back to the model as *model-only text*.
- **Removal / resolution condition:** When every Gemini model in the *model
  registry* confines its reasoning to `thought` parts. Verify by sampling a
  multi-round tool-using turn per enabled Gemini model. Count the responses
  whose parts include a non-`thought` `text` part alongside a `functionCall`.
  The bar is the weakest routed Gemini, not the newest, so a new flagship is not
  on its own evidence. At zero across at least 15 samples per model, drop the
  branch and let `content` carry the text like the other two providers. That
  also retires the `model_only_text` field, if no other provider has claimed it
  by then.
- **Status:** active. The suppression predates this entry. It is registered now
  because the fix that split its two halves made both of them explicit. The
  half that was never intended (erasing the text from the model's own history)
  is a defect and has been fixed, not tolerated.

### `generate_image` vision-misuse guard

- **Added:** 2026-06-30 (registered by the temporary-measures survey — the guard
  predated this row)
- **Lives in:** `crates/lucidos-engine/src/engine/tools/image.rs`
  (`looks_like_description_prompt` + the early-return block in
  `execute_generate_image`, plus the colocated tests
  `looks_like_description_prompt_blocks_describe_variants` and
  `generate_image_tool_description_warns_against_vision_misuse`), paired with the
  anti-misuse warning baked into the `generate_image` tool description
  (`crates/lucidos-engine/src/llm/tools/images.rs`, `get_image_generation_tool`).
- **Impermanent because (tolerates):** The model sometimes calls `generate_image`
  with a description/analysis prompt ("describe this image in detail") expecting
  back text, mistaking the image-*synthesis* tool for a vision tool. The provider
  would then synthesise a useless derivative image of nothing. The guard detects
  description-shaped prompts and returns an error pointing the model at its native
  vision / `view_image` instead. The tool-description warning came first and the
  misuse kept recurring (real prompts "observed in the wild" — see
  `looks_like_description_prompt_blocks_describe_variants`), so guidance alone was
  insufficient. We would not want the guard if the model were perfect.
- **Removal / resolution condition:** When the model reliably stops calling
  `generate_image` with description/analysis prompts — sample a batch of recent
  `generate_image` calls; if none are description requests over a representative
  window, drop the `looks_like_description_prompt` guard + its tests and restore
  the plain "prompt is required" path, and relax the anti-vision-misuse note in the
  tool description.
- **Status:** active

### "Did it again" anti-hallucination rule (repeat action without a fresh tool call)

- **Added:** 2026-06-30
- **Lives in:** `crates/lucidos-engine/src/engine/chat/process/system_prompt.rs`
  (`REPEATED_ACTION_RULE` const + the generalized CRITICAL RULE #1 it sits
  beside, spliced via the `__REPEATED_ACTION_RULE__` placeholder; the separate
  VERIFICATION line folded into that rule on 2026-08-07),
  with the colocated test `chat_prompt_forbids_faking_repeated_actions` in
  `crates/lucidos-engine/src/engine/chat/process_tests.rs`.
- **Impermanent because (tolerates):** On a repeat request — the user says
  "again" / "once more" / "send another" — the chat model pattern-matches on an
  identical earlier `ToolUse` (e.g. `send_notification`, `send_email`, `events
  emit`) sitting in its context and writes the confirmation ("Sent another") for
  the new turn WITHOUT actually calling the tool, so the action never happens but
  the user is told it did. Observed in the testing-notifications thread
  (`a09253af`, 2026-06-30): four "again"s, but `send_notification` fired only on
  the first two — the 3rd and 4th turns streamed "Sent another …" with no
  `ToolCalled`. The CRITICAL RULES block already warned against claiming an
  un-done action, but only for `write_file`/`edit_file`, so the model didn't
  apply it to other action tools. Pure model-tolerance: a perfect model would
  re-invoke the tool whenever it claims a fresh action.
- **Removal / resolution condition:** When the model reliably re-invokes the
  action tool on repeat requests — sample a batch of threads where the user asked
  to repeat a `send_notification` / `send_email` / `events emit` action; if every
  confirmation is backed by a matching `ToolCalled` in the SAME turn over a
  representative window, drop `REPEATED_ACTION_RULE` + its placeholder + the test,
  and narrow CRITICAL RULE #1 back to file writes.
- **Status:** active

### "Narrating it does not do it" on an event-wait re-arm

- **Added:** 2026-08-06
- **Lives in:** `crates/lucidos-engine/src/engine/event_wait/mod.rs` (the final
  sentence of the `WAIT_SPENT_NOTICE` const, "Narrating it does not do it: a turn
  that ends with no new call leaves nothing watching for this, whatever the
  sentence said") and `crates/lucidos-engine/src/llm/tools/misc.rs` (the
  `await_event` description's "Saying you will re-subscribe is not
  re-subscribing", shortened from the fuller sentence by the 2026-08-07
  schema-budget trim; the wake notice still carries the long form).
  Pinned by `the_delivery_wake_says_the_subscription_is_spent_and_re_arming_is_a_call`
  in `engine/event_wait/mod_tests.rs` and
  `await_event_description_says_a_wake_spends_the_subscription`
  in `llm/tools/tests.rs`.
- **Impermanent because (tolerates):** The same "wrote the confirmation instead
  of calling the tool" mistake as the entry above, in the one place it is most
  invited. A re-arm is the LAST thing a delivery turn does, so the habit of writing
  the closing paragraph after the last tool call puts prose exactly where the
  call had to go. (Until 2026-08-06 there was a second reason, since
  `await_event` was a TERMINAL tool that ended the turn outright; ADR 0049 made
  every wait detached, so it now returns like any other tool and the turn
  carries on. The mistake outlived the shape that invited it.) Observed
  2026-08-06 in the *Notification of Agent Code Edits* thread: the thread was
  re-opened by a delivery, reported the edit, closed with "Re-arming the watch now, so
  I'll keep reporting each edit as it happens" and ended the turn with no second
  `await_event` call, leaving an idle thread that had just promised to keep
  watching. Pure model-tolerance: a perfect model told the subscription is spent
  and to call again before the turn ends does not additionally need to be told
  that saying so is not doing so.
  **It recurred on 2026-08-13**, in *Comment Sweep and Engine Restart*, the same
  way. A delivery turn closed with "I am watching for the sweep to land, and I
  will apply build-slot the moment it does", and armed nothing. The user caught
  it ("ur not actually watching tho?") against a `live_event_wait_count` of 0.
  Two occurrences is what moved the answer from a fourth sentence to a
  deterministic gate.
- **Now backed by the wake check (ADR 0071).** The agentic loop refuses to end a
  turn that leaves open *todo items* with no live wait and no background task,
  and asks once instead. That closes the case where the thread had written the
  work down. The prose still carries the case it cannot see, a claim made with
  no todo list behind it, which is why these sentences stay.
- **Removal / resolution condition:** When a delivery reliably produces
  either a re-arm call or an honest "I have stopped watching" in the same turn.
  Sample threads that took an `EventWaitDelivered` and whose reply claims to
  keep watching; if every such claim is backed by an `EventWaitStarted` in the
  SAME turn over a representative window, drop the two sentences and the
  assertions that pin them. **Sample only threads with no todo list**, or the
  wake check answers for the prose and the measure reads as removable when it is
  not. The rest of `WAIT_SPENT_NOTICE` stays either way: "this subscription is
  now spent, call `await_event` again before this turn ends" is a system fact a
  perfect model still needs, not a crutch.
- **Investigation:** none (the mistake is understood, not under investigation).
- **Status:** active

### Empty-echo stale-resume: output-shape inference for an unconfirmed attach

- **Added:** 2026-07-02
- **Narrowed:** 2026-07-29 — a confirmed attach now vetoes the whole heuristic
  (see below); what remains is the unconfirmed-attach fallback.
- **Lives in:** `crates/lucidos-engine/src/engine/agent_session/lifecycle.rs`
  (`is_stale_resume_signal` / `StaleResumeInputs` — the output-shape fields
  `result_text_empty`, `buffered_text_empty`, `no_prior_results_this_turn`,
  `no_tool_calls_this_turn`) + its feeder `tool_calls_seen` counter in
  `crates/lucidos-engine/src/engine/agent_session/run_session/run.rs`, with the
  regression tests `empty_result_on_resume_but_made_tool_calls_is_not_stale_resume`
  and `unconfirmed_attach_still_falls_back_to_the_empty_echo_heuristic` in
  `agent_session/lifecycle_tests/classify.rs`.
- **Impermanent because (tolerates):** The heuristic infers "the resumed session is
  dead" from empty assistant output on the first Result, which no model output can
  actually prove. Two false positives so far, from opposite directions: a terse
  model (Fable-5) that emits no text and jumps straight to a tool call
  (2026-07-02 demo-director — cancelled + re-spawned → duplicate CC process on the
  shared worktree, 2× quota), and a resumed transcript ending on an interrupted
  tool_use, where `claude --print --resume` emits a Result for its OWN synthetic
  `Continue from where you left off.` / `No response requested.` turn before
  reading our stdin (2026-07-29 thread `cb503361`, Opus-5 — killed 10 ms after
  Init, thread wedged at `running`). The `no_tool_calls_this_turn` condition
  covers the first; nothing about output shape covers the second.
- **Partially resolved (2026-07-29):** the structured signal this row asked for is
  now implemented as `StaleResumeInputs::resume_attach_confirmed` — both backends
  report at `Init` the session id they actually attached to, and a FAILED resume
  yields a different one (CC opens a fresh conversation; Codex falls back to
  `thread/start`). A match is structural proof of life and vetoes every
  output-shape field. That removes the entire false-positive surface for resumes
  the backend confirms.
- **Removal / resolution condition:** What remains is the **unconfirmed-attach**
  case — the backend reported a different session id, or reported none before the
  first Result — where output shape is still the only signal. Drop the remaining
  heuristic (`is_stale_resume_signal`'s four shape fields + `tool_calls_seen`) once
  a *reported-and-different* sid is treated as definitive on its own, i.e. once
  `Init` is guaranteed to precede the first `Result` on every backend so
  `init_sid != resume_sid` can decide alone without an output fallback. Verify by
  confirming Init-before-Result ordering holds for CC and the Codex app-server
  across a representative window, then key the fresh-spawn retry on the sid
  comparison plus `is_definitive_session_not_found` alone.
- **Status:** active (narrowed)

### Bare `app` href recovery (LLM link with no id)

- **Added:** 2026-06-30 (registered by the temporary-measures survey — the recovery
  predated this row)
- **Lives in:** `crates/lucidos-app/src/utils/linkifyPaths.ts` (`BARE_APP_HREF` +
  the last-resort `rewriteBareAppAnchorByText` rewriter).
- **Impermanent because (tolerates):** The LLM emits bare "open the app" hrefs —
  `[Site Publisher](app)`, `(app/)`, `(app:)` — when it knows the app name should
  be a link but supplies no id. These carry no id, so the strict `app:<id>` /
  `apps/<id>` rewriters decline and the link would otherwise render as a dead
  relative `<a href="app">` resolving against the gateway base to `/<slug>/app`.
  The recovery resolves the app from the anchor's visible TEXT instead, and runs
  only after the strict href rewriters decline so a real link is never hijacked.
  Pure model-tolerance: a perfect model would emit `app:<id>`.
- **Removal / resolution condition:** When the agent reliably emits `app:<id>` /
  `apps/<id>` links carrying an id — sample recent app-link emissions; if none use
  the bare `app` shape over a representative window, drop `BARE_APP_HREF` +
  `rewriteBareAppAnchorByText` and its last-resort call site.
- **Note (2026-07-03):** the driver — the chat system prompt line telling the agent
  to "mention the app name" so it auto-links (bare-text app-name linkification) — was
  removed (`docs/plans/2026-07-03-remove-app-name-autolink.md`). The agent is now told
  app names are plain text, so it should emit far fewer bare `app` links; this advances
  (does not yet satisfy) the removal condition above. The recovery stays as a safety net.
- **Status:** active

### Bare app-id/name href recovery (LLM link with the id AS the href)

- **Added:** 2026-07-03
- **Lives in:** `crates/lucidos-app/src/utils/linkifyPaths.ts` (`extractBareAppRef`
  + the `rewriteAppAnchorByBareRef` rewriter) and the mirror fallback in
  `crates/lucidos-app/src/components/chat/ChatExchange.tsx` (`handleLinkClick`).
- **Impermanent because (tolerates):** The LLM writes `[Habit Tracker](habit-tracker)`
  — the app id (or name) as a bare relative href, by analogy to
  `[Notifications](notifications)`. It carries no `apps/` prefix and no `app:`
  scheme, so the strict `apps/<id>` / `app:<id>` rewriters decline and it isn't a
  nav panel; left alone the browser resolves the relative href against the base
  href to a non-existent route and the engine's SPA fallback reloads the whole
  workspace (the "Opening workspace" splash on an iOS PWA). The recovery resolves
  the token from the HREF against the known app ids/names and routes the click
  through `openApp`; it runs only after the strict `apps/<id>`, nav, and artifact
  rewriters decline so a real link / reserved panel is never hijacked. Pure
  model-tolerance: a perfect model would emit `app:<id>` / `apps/<id>/index.html`.
- **Removal / resolution condition:** Shares the removal signal with "Bare `app`
  href recovery" above — when the agent reliably emits `app:<id>` / `apps/<id>`
  links carrying an id (sample recent app-link emissions; if none use the bare
  id/name-as-href shape over a representative window), drop `extractBareAppRef` +
  `rewriteAppAnchorByBareRef`, its call site in `applyCompiled`, and the
  `handleLinkClick` fallback branch.
- **Note (2026-07-03):** the chat system prompt no longer tells the agent to make an
  app name a clickable link (bare-text app-name linkification was removed —
  `docs/plans/2026-07-03-remove-app-name-autolink.md`); app names are plain text now.
  This should reduce all deliberate app-link emissions, advancing (not yet satisfying)
  the shared removal signal. The recovery stays as a safety net against the
  relative-href → whole-workspace-reload regression.
- **Status:** active

### Duplicate-key tolerant memory-extraction parse

- **Added:** 2026-06-30 (registered by the temporary-measures survey — the parse
  predated this row)
- **Lives in:** `crates/lucidos-engine/src/memory/extractor.rs` (the two-step
  `serde_json::from_str` → `serde_json::Value` → `from_value::<Vec<ExtractedFact>>`
  parse of the extraction response).
- **Impermanent because (tolerates):** LLMs sometimes emit duplicate JSON keys
  (e.g. `"topic"` twice) in extraction output. serde's derived `Deserialize`
  rejects duplicate struct fields, but `serde_json::Value` accepts them with
  last-wins semantics — so parsing to `Value` first and then `from_value` survives
  the model's duplicate-key habit. A perfect model would emit each key once and we
  would deserialize the struct directly from the string.
- **Removal / resolution condition:** When extraction responses reliably contain no
  duplicate keys — sample a batch of recent memory-extraction outputs; if none
  repeat a key over a representative window, drop the intermediate `Value` step and
  deserialize `Vec<ExtractedFact>` directly with `serde_json::from_str`.
- **Status:** active

### Inline tool-call XML repair (`<invoke name="...">` as text)

- **Added:** 2026-06-30
- **Lives in:** `crates/lucidos-engine/src/engine/inline_tool_call_repair.rs` +
  its wiring in `crates/lucidos-engine/src/engine/agentic_loop/run.rs`
  (mid-stream suppression in the token callback + the post-response
  `tool_call_repair` block).
- **Impermanent because (tolerates):** The chat model sometimes emits a tool call
  as inline `<invoke name="TOOL"><parameter name="K">V</parameter>...</invoke>`
  XML *text* (its training-time tool-call serialization) instead of a structured
  `tool_use` block. The agentic loop then sees `tool_calls` empty and persists the
  raw XML as the turn's `ResponseGenerated` (observed in a release-polling thread:
  a `bash_output` poll written as text, terminating the turn with the XML — incl.
  a stray task-id UUID — as the visible answer). Verified NOT an engine bug: the
  Vertex SSE parser strictly separates `text_delta` → content from
  `input_json_delta` → tool_calls, history reconstruction uses structured
  `ContentBlock::ToolUse`, and neither the prompt nor loaded context primes the
  format (see `docs/plans/2026-06-30-inline-tool-call-leak-repair.md`).
- **Removal / resolution condition:** When the model reliably stops leaking tool
  calls as text — disable the repair and confirm a sampled batch of chat turns
  shows no `<invoke name=` substrings in any persisted `TextStreamed` /
  `ResponseGenerated`. On removal, drop the module + its `run.rs` wiring + the
  suppression branch.
- **Status:** active
- **Investigation:** `model-tool-call-as-text`

### Inline `ask_user_question` XML repair (`<ask_user_question>` as text)

- **Added:** when `inline_question_repair` shipped (backfilled to the registry
  2026-06-30 — it predates this rule and was never logged).
- **Widened:** 2026-08-15, after two fresh leaks got through. See
  `docs/plans/2026-08-15-inline-question-leak-object-body-and-degenerate-tag.md`.
- **Lives in:** `crates/lucidos-engine/src/engine/inline_question_repair.rs`,
  with its tests in the sibling `inline_question_repair_tests.rs`. Wired in
  `crates/lucidos-engine/src/engine/agentic_loop/run.rs`: mid-stream
  suppression, the post-response `inline_repair` block, and the
  `QuestionReaskCause::LeakedAsText` arm of the re-ask guard.
- **Impermanent because (tolerates):** The same leak class as the entry above, for
  one specific tool: the model emits `<ask_user_question>[...]</ask_user_question>`
  as inline text instead of a structured `ask_user_question` tool call —
  collapsing a clickable question card into raw XML. Observed even after the
  `ASK_USER_QUESTION_RULE` prompt explicitly told the model not to type the tag,
  so prompt guidance alone was insufficient.
- **Body shapes tolerated:** three, all normalised to the questions array. A JSON
  array is the shape that shipped. A single-key `{"questions": [...]}` object and
  an unfenced payload of that object, alone at the end with no tag, were added
  when the measure widened. The detector shipped requiring an array, which is
  why both 2026-08-15 leaks got through: one carried the object form, the other
  bare prose. A tag whose body is not dispatchable is stripped, its prose kept,
  and a bounded re-ask forced.
- **Removal / resolution condition:** Same as above — when the model stops leaking
  the tag (sample chat turns for `<ask_user_question` in persisted text with the
  repair disabled). On removal, drop the module + its `run.rs` wiring + the
  suppression branch + the `LeakedAsText` re-ask cause.
- **Status:** active
- **Investigation:** `model-tool-call-as-text`

### HTML entities in tool-argument text (`Machine &amp; Tooling Health`)

- **Added:** 2026-08-09
- **Lives in:** `crates/lucidos-engine/src/engine/tool_arg_entity_repair.rs`
  (+ its tests in the sibling `tool_arg_entity_repair_tests.rs`) and its wiring
  in `crates/lucidos-engine/src/engine/agentic_loop/run.rs`, the third
  post-response repair block. The two guards that pin the bisection are
  `tool_argument_special_characters_survive_the_sse_accumulator`
  (`llm/anthropic_wire.rs`) and
  `tool_called_args_persist_special_characters_verbatim`
  (`engine/event_bus_tests/serialization_persistence.rs`).
- **Impermanent because (tolerates):** The chat model sometimes HTML-entity-
  escapes the text it puts in a tool argument, so a trigger group the user
  asked to call `Machine & Tooling Health` was created, persisted and re-served
  as `Machine &amp; Tooling Health`. Affects `& < > " '` alike, and the
  corruption reaches the domain event, so it is permanent in the user's data.
  Verified NOT an engine bug, from both ends: no site on the write path
  escapes (the four entity-encoding sites in the tree all build standalone HTML
  pages, and the frontend's are render-time), and the model's context for the
  canonical turn held bare `&` with zero entities in both preceding tool
  results. Two shapes rule out a transport escape outright: one
  `run_coding_agent` call carries `"title": "Nightly: build &amp; test"` beside
  a clean `"prompt": "Build & test the engine…"` in the SAME arguments object,
  and human `MessageReceived` text is clean at scale (21 bare-`&` messages
  against 1 entity, which was the user quoting the bug). See
  `docs/plans/2026-08-09-tool-arg-html-entity-repair.md`.
- **Removal / resolution condition:** When the routed models reliably stop
  escaping tool-argument text. Sample with the repair disabled, and count the
  right denominator: tool calls whose allow-listed plain-text arguments
  actually CONTAIN one of `& < > " '`, not raw turn count. A population that
  never passed one of those characters proves nothing. On removal, drop the
  module, its tests and the `run.rs` block; keep the two transport/persistence
  guards, which assert a property of our own code rather than the model's.
- **Status:** active
- **Investigation:** `model-escapes-tool-arg-text`

### code-review findings array leaking into chat (`[]` after "no findings")

- **Added:** 2026-07-01
- **Lives in:** `.claude/skills/code-review/SKILL.md` (Output section — the
  `ReportFindings`-tool / in-band-array handoff contract and its "never print the
  findings array as prose" rule) and `.claude/commands/harden.md` (Phase 1 "report
  in prose, the findings are structured data" directive).
- **Impermanent because (tolerates):** `/harden` Phase 1 runs `code-review`
  inline, so the skill's findings handoff shares the coding-agent transcript. When
  a review surfaces nothing, the model tends to **echo the empty findings array as
  prose** — a stray `` `[]` `` after "no actionable findings" (observed
  2026-07-01, thread `79813abc`). Same class as the two inline-XML-repair entries
  above: the model prints structured/handoff data as text. The fix routes findings
  through a structured channel — `ReportFindings` on Claude Code (renders
  structurally, never as text), and a `No findings.` prose sentinel instead of a
  bare `[]` on backends without the tool — so the array never enters the
  transcript at the source. A prior attempt (2026-06-10, `aad297dd5`) deleted a
  brittle frontend strip (`stripCodeReviewEmptyFence`) and bet on a prose-only
  "never paste the array" directive in `harden.md`; the model ignored it three
  weeks later, so the source-level structured handoff replaced the
  directive-alone approach. Pure model-tolerance: a perfect model would discharge
  the findings via the structured channel and never re-print them. (We
  deliberately did NOT restore a downstream content-filter — separating the
  machine-readable findings path from the chat path IS the fix; a text filter was
  the anti-pattern.)
- **Removal / resolution condition:** When the coding agents reliably stop echoing
  the findings array as text — sample a batch of recent `/harden` Phase 1
  transcripts (grep persisted `CodingAgentTextStreamed` for a standalone
  `` `[]` `` / `[]` / `{}` block); if none leak over a representative window, the
  anti-echo steering in both files is dead weight and the Output / Phase-1 wording
  can relax to a plain "report findings via the structured channel." If the leak
  proves intractable at the source, the escalation is a deterministic downstream
  strip (frontend or engine) — explicitly rejected here in favor of the source
  fix, and only reconsidered if the source fix demonstrably fails.
- **Status:** active

---

## 3. Feature flags & sunset deprecations

Feature flags / kill-switches awaiting cleanup, and back-compat shims/aliases that
carry a **concrete** removal condition (NOT permanent back-compat — that's OUT). A
flag belongs here the moment it lands; a shim belongs here only if you can name the
event that retires it.

### `self_curated_context_mode` flag (ADR 0085)

- **Added:** 2026-08-19
- **Lives in:** `crates/lucidos-engine/src/core/preferences.rs` (the key, the
  two schedule keys and their readers), `core/preference_catalog.rs` (the three
  settable rows), and `engine/chat/process/context_mode.rs`, which owns the
  whole mode. Also the `todo_write` gate in `llm/tools/misc.rs`.

  The rest is spread thin on purpose. `context_panel.rs` renders the panel,
  `working_understanding.rs` parses and renders the document and holds
  `ThreadEvent::WorkingUnderstandingWritten` and `ContextKeptOpen`. The agentic
  loop takes a `CuratedTurn`, runs the sweep and appends the tail.
  `engine/context.rs` carries `ProtectedAddresses`, `TrimGuards` and the
  mode-aware recovery clause. `llm/anthropic_wire.rs` anchors its cache marker
  in front of a `ContentBlock::EngineTail`, and `SummaryPlan::refresh_boundary`
  holds the summariser gate.
- **Impermanent because:** it is one arm of an experiment, not a setting anyone
  is meant to live with. The mode ships dark so the benchmark can measure it
  against a baseline. Until that measurement is read, "on" is a claim nobody has
  evidence for, and the flag exists only to make the two comparable.
- **The default inverted, and the flag did not move.** ADR 0085's second
  amendment made a body leave at the end of the round it arrived on, unless the
  model kept it. That is a treatment change under ADR 0087's decision 14,
  distinguished by the `guidance_hash`. So the flag is still one arm of one
  experiment, and the bar it has to clear is unchanged.
- **The first run at the ceiling did not answer it.** Run
  `07e4aa2ef0bc4317952150e4e363f433` recorded zero `ContextKept` and zero
  `ContextDismissed` in 206 rounds. ADR 0103 found the blind trimmer had
  destroyed the lean arm's context on the hardest task. So the run cannot
  separate a wrong default from a sabotaged arm. It fixes the trimmer and leaves
  the flag where it is, for one more run.
- **The bar it was measured against is gone, and the condition is rewritten.**
  ADR 0110 supersedes ADR 0087 and retires the graduation and kill conditions.
  Nothing computes a verdict now, so "the eval reports against the bar" is a
  condition nothing can satisfy. A condition nobody can meet is how a temporary
  measure becomes permanent quietly, which is what this registry exists to stop.
- **What the flag gates was rebuilt, and the flag itself did not move.** The
  one-round rule became the *swept window*, the scratchpad became the *working
  understanding*, and the keep verb became a `[KEEP OPEN]` line. The key was
  renamed from `context_mode_experimental` in the same change, because no
  workspace had ever set it. The record is
  `docs/plans/2026-08-24-the-working-understanding-and-the-ten-round-window.md`.
  The flag stays one arm of one experiment throughout.
- **Removal / resolution condition:** a benchmark run at the full window and at
  the budgets below it, in both arms, read by a human who then decides. Kept:
  delete the flag, the catalog row, the branches and the baseline paths, leaving
  the mode unconditional. Dropped: delete the flag, the mode, the keep verb, the
  working understanding, the context panel and the gate row, leaving today's
  behaviour. `dismiss_from_context` went with ADR 0109 and is not coming back
  either way.
- **The benchmark does NOT go with it.** ADR 0087 promised
  `crates/lucidos-eval` and `eval/context-mode/` would be deleted alongside the
  flag, because they existed to decide it. Under ADR 0110 they measure how any
  configuration handles its context, which outlives this decision. What goes
  with the flag is `Arm`, its preference rows, and the paired path.
  **Anything else leaves the mode experimental and this row open.** Verify with
  a grep for the three keys across `crates/**`, `system-knowhow/**` and
  `eval/**`, which must return nothing but this row and `Arm::preference_rows`.
- **Status:** active

### `lucidos spawn-thread --parent` deprecated alias

- **Added:** 2026-06-30 (registered by the temporary-measures survey — the alias
  predated this row)
- **Lives in:** `crates/lucidos-cli/src/main.rs` (the `parent: bool` arg, marked
  `DEPRECATED — alias for --relation child … Will be removed in a future release`)
  + `crates/lucidos-cli/src/spawn_thread.rs` (the relation-resolution arm that maps
  `--parent` → `CliRelation::Child` and prints the stderr deprecation warning). The
  contract is pinned by `parent_flag_still_works_with_deprecation_warning` in
  `crates/lucidos-cli/tests/spawn_thread_posts_body.rs`.
- **Impermanent because:** `--parent` is a deprecated alias for `--relation child`,
  kept only so existing recipes / scripts that still pass `--parent` keep working
  for one release while the stderr warning nudges callers to migrate. It is NOT
  permanent back-compat — it carries an explicit "Will be removed in a future
  release" sunset. (The sibling `sub` alias for `child` is, by contrast, undated
  permanent back-compat and is deliberately NOT tracked here.)
- **Removal / resolution condition:** One release after the deprecation warning
  shipped, once callers have had a release cycle to migrate — verify nothing in the
  tree still passes `--parent` (grep the repo + `system-knowhow/**` and any
  workspace recipes for `--parent`), then remove the `parent` arg, the
  relation-resolution arm + warning in `spawn_thread.rs`, and the
  `parent_flag_still_works_with_deprecation_warning` test.
- **Status:** active

### `email:`-prefixed credential fallback in `get_email_password`

- **Added:** 2026-08-05
- **Lives in:** `crates/lucidos-engine/src/core/credentials.rs`
  (`CredentialStore::get_email_password`: the
  `service_name = $1 OR service_name = 'email:' || $1` disjunction and its
  `ORDER BY (service_name = $1) DESC` preference for the unprefixed row), pinned by
  `migration_strands_an_email_row_whose_bare_name_is_taken`.
- **Impermanent because:** `20260805134838_drop_credential_name_prefixes_use_auth_type.sql`
  strips the `email:` prefix so `auth_type = 'email_password'` is the only thing
  marking a mailbox password. It cannot strip EVERY row: `email_password` lives
  under the globally-unique arm of the new constraints, so a workspace holding both
  an `email:work` mailbox password and a separate `work` API key would have the two
  collide on one name. The migration leaves that row prefixed rather than aborting
  and blocking engine startup, and this fallback is what keeps the stranded row's
  mailbox working until the user resolves the duplicate. It is NOT permanent
  back-compat: the migration names every skipped row in a `RAISE NOTICE`, so the
  set is finite, known, and shrinking.
- **Removal / resolution condition:** Once no workspace has an `email_password` row
  whose `service_name` still starts with `email:`. Verify with
  `SELECT service_name FROM credentials WHERE auth_type = 'email_password' AND service_name LIKE 'email:%'`
  against each live workspace database, expecting zero rows, then drop the `OR` and
  the `ORDER BY` from the query, and drop the stranded-row half of
  `migration_strands_an_email_row_whose_bare_name_is_taken` (keep the half asserting
  the migration does not clobber the same-named API key).
- **Status:** active

### `oauth:` prefix stripped from a caller-supplied credential name

- **Added:** 2026-08-05
- **Lives in:** `crates/lucidos-engine/src/core/oauth.rs`
  (`client_provider_name`, the `strip_prefix("oauth:")`), reached from three
  sites: the `request_credential` LLM tool
  (`engine/tools/credentials.rs::requested_service_name`),
  `POST /api/v1/credentials` (`api/settings.rs::create_credential`), and the
  proxy's `api/proxy.rs::fetch_required_credential`, whose fallback covers a
  `data/config/apis.json` entry that still names a credential
  `oauth:<provider>`. That third site is the load-bearing one: `data/config/` is
  user data no DB migration can rewrite, so a live config would otherwise 502 on
  every request the moment the prefix migration runs (a live workspace
  had two such entries when this shipped). Pinned by
  `client_provider_name_strips_a_legacy_prefix`,
  `fetch_required_credential_tolerates_a_legacy_oauth_prefixed_name`, and
  `fetch_required_credential_still_reports_a_genuinely_missing_one`.
- **Impermanent because:** an `oauth_client` credential is named for the provider
  alone now. The strip exists only because `oauth:<provider>` is still in
  circulation as the spelling agents and workspace knowhow learned: the chat system
  prompt advertised it for as long as the tool existed. A caller passing either
  spelling has to land on ONE row, because the failure mode is the 2026-08-05
  incident, where a mismatch left the user holding two credentials for one provider
  and instructions to delete one by hand. This is a model-and-recipe tolerance
  measure, not a naming rule: we would not want it if every caller read the current
  docs.
- **Removal / resolution condition:** Two independent halves, removable
  separately. For the proxy fallback: once no live workspace's
  `data/config/apis.json` names a credential `oauth:<provider>` (grep each
  workspace's `data/config/`), drop the second attempt in
  `fetch_required_credential` and its two tests, keeping the first attempt's
  `get_oauth_client` leg, which is a real capability rather than a tolerance.
  For the write-path strip: once no shipped prompt, `system-knowhow/**` file, or
  workspace recipe still says `oauth:<provider>` (grep the tree and the live
  workspaces' `data/knowhow/`), AND a sampling of the weakest models in the
  *model registry* passes a bare provider name to `request_credential` unprompted
  (see § 2's sampling rule), drop the `strip_prefix` and the
  `client_provider_name_strips_a_legacy_prefix` test, keeping the lowercase/trim
  normalization, which is a real rule rather than a tolerance.
- **Status:** active

### `script_handshake` workspace-root script fallback

- **Added:** 2026-07-01
- **Lives in:** `crates/lucidos-engine/src/api/proxy_script_runner.rs`
  (`run_handshake_script` — the `data_abs` → `root_abs` resolution: try
  `<workspace>/data/<script>` first, then fall back to `<workspace>/<script>`),
  pinned by `data_relative_script_is_found` + `data_relative_preferred_over_workspace_root`.
- **Impermanent because:** A `script_handshake` `script` path is now resolved
  relative to `data/` (the git-tracked, documented location — the fix for
  doc-following scripts 404'ing). The workspace-root fallback exists ONLY so a
  handshake script placed at `<workspace>/<script>` before this fix — the location
  the engine used to look, which is outside version control — keeps working through
  the transition. It carries a sunset (scripts should live under `data/`), so it is
  NOT permanent back-compat.
- **Removal / resolution condition:** Once handshake scripts are confirmed to live
  under `data/` across deployments — verify no live workspace has a
  `script_handshake` `script` resolving via the root fallback (i.e. present at
  `<ws>/<script>` but absent at `<ws>/data/<script>`) — drop the `root_abs`
  fallback branch so resolution is `data/`-only, keep `data_relative_script_is_found`,
  and remove `data_relative_preferred_over_workspace_root` (the root leg of the
  precedence test). This is a deliberate breaking change; schedule it with a release
  note.
- **Removed:** 2026-08-27. This removal discharged the condition first, across
  every workspace on the release machine. One carried `script_handshake` entries,
  and both of its scripts sat under `data/`, so nothing live depended on the fallback.
  `run_handshake_script` now joins `data/` alone, and reports `NotFound` on that
  path when the file is absent. `a_script_at_the_workspace_root_is_not_run` replaces
  the precedence test, and the runner's two test helpers collapsed into one. The
  release note is in `CHANGELOG.md`, and `system-knowhow/building-an-auth-handshake.md`
  no longer names the root as a place a script may sit.
- **The audit above answered a different question, and the removal broke a live
  workspace.** The condition asks for a script "present at `<ws>/<script>` but
  absent at `<ws>/data/<script>`". The audit asked whether the scripts lived
  under `data/`. For a config spelling its value `data/scripts/auth/x.py`, both
  are true at once: the file is under `data/`, and `<ws>/data/<script>` resolves
  to `data/data/...`, which is absent.

  That workspace shipped in 0.32.0 with every handshake proxy dead. Superseded
  by `config_path_under_data`, which strips one redundant `data/` so both
  spellings resolve, with the `data/`-only property intact. See
  `docs/plans/2026-08-29-handshake-path-spelling-and-injected-secret-binding.md`.
- **Status:** removed (2026-08-27), superseded by a normalizer (2026-08-29)

### Scheduler one-time startup migrations (`migrate_db_triggers_to_events`, `migrate_stale_trigger_prompts`)

- **Added:** 2026-07-02 (registered by the /harden-project sweep — both migrations
  predate this row)
- **Lives in:** `crates/lucidos-engine/src/scheduler/mod.rs` —
  `migrate_db_triggers_to_events` (landed 2026-04-06, commit 02515f4d9) and
  `migrate_stale_trigger_prompts` (landed 2026-04-07, commit d32b2a518), both
  called once from `start()`; both carry `DEPRECATED` doc comments naming their
  own removal plan.
- **Impermanent because:** One-time startup migrations — the first migrates legacy
  `trigger_crons` rows into events, the second rewrites stale placeholder trigger
  prompts. Dead-on-arrival for any install created after they landed; they run as
  no-ops on every subsequent boot.
- **Removal / resolution condition:** Confirm every live install has started up at
  least once since 2026-04-07 (or wait for the next release that requires a fresh
  install / telemetry confirming zero workspaces retain the legacy table or
  placeholder prompts). Then drop both functions, their call sites in `start()`,
  and the `ScheduledTrigger*` event aliases in `triggers/replay.rs` (named by the
  first migration's comment).
- **Status:** active

### Superseded turn-control localStorage keys cleared at load

- **Added:** 2026-08-11
- **Lives in:** `crates/lucidos-app/src/store/store.ts`, the two
  `localStorage.removeItem('lucidos-steps-expanded' / 'lucidos-details-expanded')`
  calls immediately below the `stepsExpanded` / `detailsExpanded` seeds.
- **Impermanent because:** A one-shot purge of two keys nothing writes any more.
  The *turn controls* flipped to defaulting ON, and the old keys could not be
  reused: the previous persisting effect wrote its value on every load, clicked
  or not, so every browser that had opened the app held a `false` recording the
  old default rather than a reader's choice. The seeds moved to `-v2` names and
  these two calls clear the dead pair so nobody reads a stale `false` as the
  state of a control that is on. Dead on arrival for any browser profile created
  after 2026-08-11, and a no-op on every load after the first.
- **Removal / resolution condition:** Drop the two calls once every device that
  opened this workspace before 2026-08-11 has loaded the app at least once since
  (in practice: a release boundary past which the pre-`-v2` keys cannot exist).
  Nothing else goes with them. The `-v2` key names are permanent, and
  `persistTurnControl`'s deviation-only write is the design that keeps a stored
  value meaning "the reader turned this off", not a crutch: neither is part of
  this measure.
- **Status:** active

### `repo` → `folder` deprecated alias on `run_coding_agent`

- **Added:** 2026-07-02 (registered by the /harden-project sweep — the alias
  landed 2026-05-25/27, commits 713e33b1d/976a4a516)
- **Lives in:** `crates/lucidos-engine/src/llm/tools/threads.rs` (the `repo` param
  schema, marked "DEPRECATED, use `folder`. Passing both is an error"; the
  "accepted for one release" sunset moved into this row on 2026-08-07, when the
  schema-budget trim cut the param description back to its rule),
  `crates/lucidos-engine/src/engine/agentic_loop_special_tool.rs`
  (the alias-resolution arm + the both-passed error, two sites), and
  `crates/lucidos-engine/src/engine/http/workspace_client.rs`
  (`CrossWorkspaceSpawn.repo`).
- **Impermanent because:** `repo` is a deprecated alias for `folder` on the
  `run_coding_agent` LLM tool, kept for one release so existing knowhow/recipes
  that still pass `repo` keep working while the schema description steers models
  to `folder`. Explicit "accepted for one release" sunset — not permanent
  back-compat. The sibling of the registered `--parent` CLI alias.
- **Removal / resolution condition:** One release after shipping (already elapsed
  as of v0.11.0+ — registration is late), verify no `system-knowhow/**`, workspace
  knowhow, or recipes still pass `repo` to this tool, then drop the schema param,
  the alias-resolution arm and both-passed error, and the
  `CrossWorkspaceSpawn.repo` field.
- **Status:** active

### `tailscale serve` pre-1.52 positional-syntax fallback

- **Added:** 2026-08-02
- **Lives in:** `crates/lucidos-app/src/mobile.rs` (`serve_arg_forms` returns
  `(current, legacy)`, `serve_run` runs the legacy form only on a
  `ServeAttemptError::FlagRejected` from the current one, and
  `both_serve_forms_failed` merges the two errors), pinned by
  `the_legacy_serve_form_is_the_pre_rework_syntax_without_bg`,
  `a_double_failure_keeps_the_legacy_reason_too`,
  `an_identical_double_failure_is_not_said_twice`,
  `a_syntax_rejection_is_told_apart_from_every_other_failure` and
  `a_rejected_flag_is_the_only_failure_that_asks_for_the_older_syntax`.
  `SERVE_CONFIGURE_TIMEOUT` / `supervise_serve` exist partly for it (the legacy
  form runs foreground on a CLI that still parses it), but the deadline is a
  general stall guard and stays behind. Why the flag asymmetry is correct:
  `docs/code-review-priors.md`.
- **Narrowed 2026-08-02:** the retry originally fired on ANY failure of the
  current form. On a modern CLI that made every unrelated failure collect the
  legacy attempt's "the CLI for serve and funnel has changed" and lead with it,
  which is how a run that was really waiting for a tailnet approval got reported
  as a syntax problem. It is now gated on `is_flag_rejection`, so a deadline, a
  cancel, or a daemon that is down never reaches it. That gate is part of the
  measure and is deleted with it.
- **Impermanent because:** The Expose button's real invocation is the flag form
  `serve --bg --https=443 <target>`, which every Tailscale CLI from the 1.52
  rework onward accepts. The positional `serve https / <target>` second attempt
  is there only for a Mac still on a pre-1.52 CLI. Unlike a persisted wire shape
  or an old event payload, an installed CLI gets upgraded, so the population
  this serves shrinks to zero on its own. It landed because the reverse
  hardcoding (positional only) is exactly what broke Expose once upstream
  removed that syntax, so a single hardcoded form is what we are avoiding.
- **Removal / resolution condition:** Once a pre-1.52 CLI is no longer worth
  supporting (1.52 is already the floor the README and
  `system-knowhow/remote-access.md` teach), drop the second tuple element so
  `serve_arg_forms` returns one `Vec<String>`, collapse the fallback branch in
  `serve_run` to a single `run_serve_attempt(...)?`, delete
  `both_serve_forms_failed`, `is_flag_rejection` and the
  `ServeAttemptError::FlagRejected` variant, and delete all five pinning tests.
  Verify on the oldest CLI still in scope: `tailscale serve --help` lists
  `--https` and `--bg`. Also drop the two-form table in `remote-access.md`
  § Route B, the paragraph on reporting both errors, the paragraph on the
  flag-parse gate, and the Expose troubleshooting row that points at it.
- **Status:** active

---

## 4. Open investigations (parents)

An investigation is the *reason* a measure exists, not a measure itself — so it has
no "Lives in" site of its own. Measures in the sections above reference an
investigation by its id. The payoff: **closing an investigation surfaces every
measure now eligible for removal** — search this file for the id to find them all.

### `ios-pwa-blackout` — iOS PWA black-screen / silent content-process kill

- **Opened:** 2026-05-18
- **Lives in:** n/a (investigation)
- **Impermanent because:** An investigation closes once its question is answered.
  iOS Safari/WKWebView in standalone PWA mode intermittently kills the content
  process (or resumes to a black screen) without firing `pagehide`/`beforeunload`,
  so the failure is invisible to ordinary lifecycle hooks. The diagnostics built to
  chase it (see measures below) exist only until the cause is understood and fixed.
- **Removal / resolution condition:** Root cause identified and fixed, confirmed by
  the absence of fresh `kind:"likely_crash"` breadcrumbs (and no new black-screen
  reports) over a representative iOS-PWA usage window. On close, flip every measure
  tagged `Investigation: ios-pwa-blackout` to `removed` per its own removal steps.
- **Resolved:** 2026-06-30 — two distinct symptoms, both answered. (1) The
  blank-body half is compositor **paint loss** (DB has the data; JS renders fine;
  only the WKWebView layer texture is stale), root-caused by the render probe
  (24/24 `content-present`) and fixed by the repaint-escalation campaign
  (`forceIOSRepaint` scroll-nudge + forced layout read, extended burst tail,
  settle re-burst, deep-link-claim deferral — 2026-06-27→06-30). (2) The
  black-screen / content-process-kill half is quiet: **0** `kind:"likely_crash"`
  in both live workspaces across their full multi-week log spans; the only one
  anywhere is a single `dev` hit dated 2026-06-15, predating the fixes. **The
  diagnostics are RETAINED, not removed** (see the two measures above) — judged
  reusable client-stability tooling rather than blackout-only dead weight, so this
  close deviates from the original "delete the modules" step.
- **Status:** resolved (2026-06-30)
- **Recurrence:** the same paint loss surfaced on the packaged Mac app, which is
  WKWebView. The root cause here stands; what was missing was coverage, because
  every recovery lever was gated on iOS. See `webkit-desktop-blank-thread` below.
- **Measures referencing this investigation:** iOS-PWA liveness diagnostic (§1),
  Thread-render blank-body probe (§1) — both resolved 2026-06-30, code retained.

### `webkit-desktop-blank-thread`: a thread opens blank in the packaged Mac app

- **Opened:** 2026-08-26
- **Lives in:** n/a (investigation)
- **Impermanent because:** an investigation closes once its question is
  answered. A thread opens with an empty transcript in the packaged macOS app,
  around three opens in ten, and the content appears on the first scroll. The
  title, the up chevron and the composer all draw, so the transcript is in the
  DOM and scrollable. That is the signature `ios-pwa-blackout` closed on, which
  is WebKit serving a stale layer texture. The fix widened every repaint gate
  from iOS to the engine
  (`docs/plans/2026-08-26-the-repaint-recovery-covers-every-webkit-client.md`).
- **Removal / resolution condition:** no blank open over a representative usage
  window, on a packaged build carrying the widened gate. If one DOES survive,
  the widened probe's class decides the next move. A `content-present` line
  means the nudge needs escalating on this surface. Any render-side class moves
  the hunt back into the store.
- **Status:** open
- **Measures referencing this investigation:** The render probe's WebKit
  surface (§1).

### `cc-reasoning-dormant` — CC coding-agent "Thinking" step shows nothing

- **Opened:** 2026-06-30 (root cause corrected 2026-07-02 — it is NOT
  Vertex-specific; the entry was formerly `cc-reasoning-dormant-on-vertex`).
  Scope narrowed 2026-07-07: **Claude Code only.** The Codex half of the same
  feature is live — codex's default `model_reasoning_summary` emits no
  reasoning notifications, so both Codex drivers now request `detailed`
  (`CODEX_REASONING_SUMMARY`, `runtime/codex.rs`) and Codex threads stream
  reasoning summaries into `CodingAgentThoughtStreamed`.
- **Lives in:** n/a (investigation). The feature code is correct and stays — the
  `CodingAgentThoughtStreamed` capture in
  `crates/lucidos-engine/src/runtime/claude_code_parse.rs` (`stream_event` arm) and
  the timeline render. Nothing here is a removable crutch; the paired parser
  field-name correction (`delta.text` → `delta.thinking`) is a permanent fix.
- **Impermanent because:** The `CodingAgentThoughtStreamed` feature (surface the
  model's reasoning as a live "Thinking" step instead of a frozen "Working", per
  `docs/plans/2026-06-25-surface-coding-agent-reasoning-in-timeline.md`) is wired
  end-to-end but produces **zero events** for the current models. Anthropic's
  `thinking.display` defaults to `"omitted"` on every current model — Fable 5 /
  Opus 5 / Opus 4.8/4.7 / Sonnet 5 — so thinking blocks stream with EMPTY text
  (encrypted signature only) and no `thinking_delta` arrives. **Opus 5 does not
  resolve it** — re-checked 2026-07-25 against Opus 5 specifically, not CC in
  aggregate: the dev workspace has 15 CC threads whose selected model is
  `claude-opus-5*`, carrying 955 `CodingAgentTextStreamed` events and **zero**
  `CodingAgentThoughtStreamed`. That is the expected result, because the block is
  upstream Claude Code's headless `stream-json` path and is model-independent — so
  no future model is expected to resolve it either. (Every thought event on record
  in that workspace is Codex.) **This is NOT Vertex-specific** — the original heading
  and premise were wrong. Verified empirically 2026-07-02 by driving the `claude`
  CLI on the **first-party Anthropic API** (`CLAUDE_CONFIG_DIR=~/.claude-personal`,
  Claude Max subscription): the headless `--output-format stream-json` stream
  carried one `signature_delta` and **zero** `thinking_delta`, identical to Vertex,
  both WITH and WITHOUT `--thinking-display summarized`. Two compounding reasons it
  stays empty: (1) the raw chain of thought is never returned for these models — a
  `"summarized"` display yields a summary at most, never the real reasoning; and
  (2) even that summary does not come through Claude Code's headless `stream-json`
  path (upstream CC limitation — GitHub anthropics/claude-code#7840, #56356,
  #49708). Orchestrators that call the Anthropic API *directly* can set
  `thinking.display: "summarized"` and get the summary; Lucidos drives the CC CLI,
  which drops it — so **switching CC's provider does NOT fix this**. A
  hoped-temporary limitation, not a permanent design choice — hence an
  investigation, not an "accepted non-bug" (which would be OUT).
- **Companion fix (2026-07-02):** because the reasoning text can't be surfaced, a
  backend-independent `REASONING_NOT_VISIBLE_RULE` was added to the coding-agent
  system prompt (`engine/agent_session/prompts.rs`, injected at the shared
  `append_backend_rules` chokepoint) telling the agent its reasoning is not shown,
  so it must put user-facing content (draft copy, the options behind a question) in
  a visible message rather than referencing invisible reasoning. This does not
  surface the reasoning; it stops the agent from *relying* on the invisible channel
  for must-see content — the real "Caption copy: do the six lines above work?" card
  whose six lines never appeared.
- **Removal / resolution condition:** When a CC turn actually lands non-empty
  `CodingAgentThoughtStreamed` events in the events table — i.e. Claude Code starts
  forwarding summarized thinking through the headless `stream-json` path (re-test:
  run a CC turn with a heavy-reasoning prompt and confirm non-empty events). On
  resolution, drop this investigation and the "dormant" notes in
  `system-knowhow/coding-agent-events.md` and the `claude_code_parse.rs` comment.
  The `REASONING_NOT_VISIBLE_RULE` companion is a permanent behavioral guard and
  stays regardless. The deferred alternative — an engine-driven elapsed
  "Thinking… (Ns)" indicator that needs no reasoning text — remains the fallback if
  CC never surfaces it.
- **Status:** open
- **Measures referencing this investigation:** none (both the parser field-name
  correction and the `REASONING_NOT_VISIBLE_RULE` companion are permanent fixes,
  not removable measures).

### `click-lag` — button/drawer clicks slow to register (dev workspace)

- **Opened:** 2026-06-24 (registered 2026-06-29 by the nightly harden sweep)
- **Lives in:** n/a (investigation)
- **Impermanent because:** An investigation closes once its question is answered.
  Button and drawer clicks intermittently feel slow to register in the dev
  workspace; the cause — a main-thread blocker (long JS re-render, forced layout,
  or paint loss) — is not yet pinned. The probe built to chase it (see measure
  below) exists only until the cause is understood and fixed.
- **Removal / resolution condition:** Root cause identified and fixed, confirmed by
  the absence of fresh threshold-crossing `[perf-probe]` lines during normal
  dev-workspace interaction. On close, flip every measure tagged
  `Investigation: click-lag` to `removed` per its own removal steps.
- **Resolved:** 2026-06-30 — retired without a pinned root cause: the lag is no
  longer perceived in normal use. The `[perf-probe]` is console-only (never posted
  to the engine), so there is no server-side signal to confirm against — the close
  rests on the absence of the symptom. **The probe is RETAINED, not removed** (see
  the measure above): it is fully general perf instrumentation, kept permanently
  registered (silent below its thresholds) as debug tooling, so this close
  deviates from the original "delete the module" step.
- **Status:** resolved (2026-06-30)
- **Measures referencing this investigation:** Click-lag main-thread blocker probe
  (§1) — resolved 2026-06-30, code retained as permanent debug tooling.

### `model-tool-call-as-text` — model emits tool calls as inline XML text

- **Opened:** 2026-06-30
- **Lives in:** n/a (investigation)
- **Impermanent because:** An investigation closes once its question is answered.
  The chat model intermittently emits a tool call as its training-time XML
  serialization — `<invoke name="...">...</invoke>` or the
  `<ask_user_question>...</ask_user_question>` special case — as plain *text* in
  the content channel instead of as a structured `tool_use` block. Confirmed to be
  a model-side leak, not an engine parse/priming bug (the Vertex SSE parser keeps
  text and tool_use strictly separate; history reconstruction is structured). The
  repairs that tolerate it (see measures) exist only until the model stops doing
  it.
- **Removal / resolution condition:** When a sampled batch of chat turns shows no
  `<invoke name=` / `<ask_user_question` substrings in persisted `TextStreamed` /
  `ResponseGenerated` with the repairs disabled. On close, flip every measure
  tagged `Investigation: model-tool-call-as-text` to `removed` per its own removal
  steps.
- **Status:** open
- **Measures referencing this investigation:** Inline tool-call XML repair (§2),
  Inline `ask_user_question` XML repair (§2).

### `model-escapes-tool-arg-text`: model HTML-entity-escapes tool-argument text

- **Opened:** 2026-08-09
- **Lives in:** n/a (investigation)
- **Impermanent because:** An investigation closes once its question is
  answered. The chat model intermittently HTML-entity-escapes text it places in
  a tool argument (`&amp;`, `&lt;`, `&gt;`, `&quot;`, `&#39;`), per-field and
  per-turn rather than consistently: one turn created seven trigger groups with
  `&amp;` in their names while a later turn renamed the same groups cleanly.
  Confirmed model-side, not an engine escape (see the measure for the full
  bisection). Sibling in spirit to `model-tool-call-as-text`, but a distinct
  question: that one is the model choosing the wrong *channel* for a tool call,
  this one is the model corrupting the *payload* of a correctly-structured one,
  so closing either says nothing about the other.
- **Open question this still owes an answer to:** whether the same escaping
  reaches tools we do not own. The Claude Code path shows the identical shape in
  `CodingAgentToolCalled` args going back to 2026-03-18, including mangled Rust
  generics (`impl.*From&lt;&amp;str&gt;` as a Grep pattern). Those calls execute
  inside CC's own process against CC's Grep/Edit, so Lucidos cannot intercept
  them and must not rewrite the record (it would make our event disagree with
  what CC actually ran). If the escaping is one model behavior, closing this
  investigation should be able to cite clean CC tool calls too.
- **Removal / resolution condition:** When a sampled batch of chat tool calls,
  taken with the repair disabled and counted over calls whose plain-text
  arguments actually contain one of `& < > " '`, shows no entity forms. On
  close, flip every measure tagged
  `Investigation: model-escapes-tool-arg-text` to `removed` per its own removal
  steps.
- **Status:** open
- **Measures referencing this investigation:** HTML entities in tool-argument
  text (§2).

### `prompt-cache-first-of-turn-miss`: the first Claude call of a turn reads no cache

- **Opened:** 2026-08-17
- **Lives in:** n/a (investigation)
- **Impermanent because:** An investigation closes once its question is
  answered. Across 578k `ContextCaptured` rows in the `dev` workspace, mid-turn
  Claude calls read cache 99.3-99.4% of the time and write ~2.1k tokens. The
  FIRST call of a turn reads NOTHING in ~47% of cases and writes ~67.5k. It
  holds even when the previous call was seconds earlier on the same thread: at a
  matched sub-30s gap, first-of-turn misses 28.6% against mid-turn 0.6%.
- **What the data already ruled out** (do not re-investigate): tool count, model
  id, elapsed time, concurrent-thread eviction, and `frontend_origin`. Section
  composition too: in 1,029 zero-read cases both cached blocks were
  byte-identical in declared size to the previous call. The 30s
  `pool_idle_timeout` is a second-order effect and cannot be the mechanism, per
  the matched-gap number.
- **The CURRENT TIME stamp is NO LONGER ruled out, and is now fixed.** The
  size evidence above was the reason it was cleared, and equal counts
  are not equal content: the wire probe caught a boundary where `system_bytes`
  held constant while `system_hash` moved, which is a fixed-width field
  changing. That is a PARTIAL read (tools survived, system did not), so it is a
  different failure from the all-or-nothing zero this entry tracks. Fixed by ADR
  0084. Re-measure before attributing any remaining zero-read case.
- **The 30-day measurement split the boundary write into three causes.**
  `data/artifacts/context-economics-investigation.md` in the `dev` workspace,
  over 19,254 `ContextCaptured` rows and 94 paired probe lines. The clock takes
  about a third and is fixed. The messages[0] rotation takes about a half,
  measured changing at 94.3% of 1,115 boundaries, and ADR 0085 removes it.
- **The ~9,200 unmatched tools tokens are RETIRED, and were never real.** ADR
  0088 and
  `docs/investigations/2026-08-18-tools-array-and-system-prompt-economics.md`
  show two arithmetic artifacts behind them. The 22,659 they came from is a
  bucket mean over 36.7% zeros. The non-zero reads in that same bucket average
  35,581, which is ABOVE the tier. The $0.058 came from a three-way residual of
  the boundary write. Do not re-derive it.
- **Reads are all-or-nothing**, now measured rather than asserted. Over 30 days
  and 1,423 boundaries, ZERO reads landed between nothing and the full tools
  tier. Each engine build reads one exact value, low equal to high. So this is
  a lookup that never matched, not a prefix that diverged partway.
- **The live question is the size of the zero-read population.** It is 56.8%
  over 30 days and 58.6% over 7, against the ~47% recorded above over all 578k
  rows. Name the window whenever you quote it. Whichever window, the tools
  array those boundaries failed to match is byte-identical on every thread and
  every workspace of that build.
- **The structural fact that shapes the search:** the tools array and the system
  prompt are built once per turn in `engine::chat::process::run` and handed to
  `run_agentic_loop` frozen. So the cached prefix cannot move mid-turn, and is
  rebuilt from scratch between turns, which is exactly where the miss lives.
- **Removal / resolution condition:** The cause is identified, and either fixed
  or attributed to Anthropic with evidence. The discriminator is two consecutive
  calls on one thread whose `[CacheProbe]` prefix hashes are identical while the
  second reads zero. That puts the miss upstream. Differing hashes name the
  segment we changed, and the bug is ours. On close, flip every measure tagged
  `Investigation: prompt-cache-first-of-turn-miss` to `removed` per its own
  removal steps.
- **Status:** open
- **Measures referencing this investigation:** Prompt-cache wire probe (§1).
