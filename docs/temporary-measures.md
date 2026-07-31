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
  (`REPEATED_ACTION_RULE` const + the generalized CRITICAL RULE #1 / VERIFICATION
  lines it sits beside, spliced via the `__REPEATED_ACTION_RULE__` placeholder),
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
  and narrow CRITICAL RULE #1 / VERIFICATION back to file writes.
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
- **Lives in:** `crates/lucidos-engine/src/engine/inline_question_repair.rs` +
  its wiring in `crates/lucidos-engine/src/engine/agentic_loop/run.rs`
  (mid-stream suppression + the post-response `inline_repair` block).
- **Impermanent because (tolerates):** The same leak class as the entry above, for
  one specific tool: the model emits `<ask_user_question>[...]</ask_user_question>`
  as inline text instead of a structured `ask_user_question` tool call —
  collapsing a clickable question card into raw XML. Observed even after the
  `ASK_USER_QUESTION_RULE` prompt explicitly told the model not to type the tag,
  so prompt guidance alone was insufficient.
- **Removal / resolution condition:** Same as above — when the model stops leaking
  the tag (sample chat turns for `<ask_user_question` in persisted text with the
  repair disabled). On removal, drop the module + its `run.rs` wiring + the
  suppression branch.
- **Status:** active
- **Investigation:** `model-tool-call-as-text`

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
- **Status:** active

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

### `repo` → `folder` deprecated alias on `run_coding_agent`

- **Added:** 2026-07-02 (registered by the /harden-project sweep — the alias
  landed 2026-05-25/27, commits 713e33b1d/976a4a516)
- **Lives in:** `crates/lucidos-engine/src/llm/tools/threads.rs` (the `repo` param
  schema, marked "DEPRECATED — use `folder` instead. Accepted for one release as
  an alias"), `crates/lucidos-engine/src/engine/agentic_loop_special_tool.rs`
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
- **Measures referencing this investigation:** iOS-PWA liveness diagnostic (§1),
  Thread-render blank-body probe (§1) — both resolved 2026-06-30, code retained.

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
