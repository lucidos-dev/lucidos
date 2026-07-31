# Code-review priors — verified non-bugs

Patterns in this codebase that **look like bugs to a fresh reviewer and are
not**. Every entry was flagged by a review agent, then dismissed by reading
the cited source. Review agents (human or LLM) should consult this file
before flagging; re-flagging an entry here requires **new evidence** (the
guard was removed, the contract changed), not re-derivation of the original
suspicion.

Maintenance: when a review dismisses a finding with evidence, add an entry
(pattern-based, not line-number-based — line numbers rot). When a code change
invalidates an entry, delete it in the same commit. Deliberate design *no*s
with deeper rationale live in `docs/adr/`; this file is for the smaller
"looks wrong, isn't" patterns that don't warrant an ADR.

## Rust engine

- **`graceful_kill_child_process_group`'s SIGTERM→sleep→SIGKILL does NOT race
  pid recycling.** The function (`runtime/spawn_env.rs`) signals a process
  *group* (`-pid`), sleeps the grace, then signals again — and a fresh reviewer
  may worry the pid (= pgid) could be recycled during the `sleep().await` and
  the second signal hit an unrelated group. It can't, at the one call site
  (`claude_code.rs` `driver_task` teardown): the call is gated by
  `if !child_reaped`, and the `tokio::process::Child` handle is held on the
  stack across the whole grace — `child.wait()`/`try_wait()` run only *after*
  the function returns. The group leader (the engine's direct child) therefore
  stays an unreaped zombie for the entire grace, so its pid cannot be recycled
  before the SIGKILL. Re-flag only if a caller starts reaping the child before
  or during the call. (`runtime/spawn_env.rs`, `runtime/claude_code.rs`.)

- **`starts_with`-guarded byte slicing is boundary-safe.** Sites like the
  Result-flush in `agent_session/run_session/run.rs` slice
  `result_trimmed[buf_trimmed.len()..]` only inside an
  `result_trimmed.starts_with(buf_trimmed)` guard — a byte-exact prefix match
  guarantees the index is a char boundary. Same for indices derived from
  `str::find()` matches (the match start/end of an ASCII needle is always a
  boundary). The "never slice by byte index" rule targets *arithmetic*
  indices, not guard-derived ones.
- **The CC fast-path has no `pending_followups` TOCTOU.** In
  `chat/process/run.rs` the session lookup, `process_exited` check, counter
  increment, send, and failure-rollback all run under one
  `agent_sessions.lock().await` — the external watchdog can't remove the
  session mid-sequence because removal takes the same lock.
- **`save_thread` / `unsave_thread` do stamp the actor.** They use
  `EventMeta::with_actor(actor)` on a `BusEvent::Thread` emit —
  `emit_user_system` is for `SystemEvent`s; `BusEvent::Thread` emits are a
  documented exception (`.claude/rules/rust.md` § "Mutating endpoints stamp
  the actor").
- **Command-guard preferences are read once per response, not per tool
  call.** See the comment block at the top of the agentic loop
  (`agentic_loop/run.rs`) — the toggles can't change mid-response.
- **Failed coding-agent turns deliberately do NOT auto-propose changes.**
  `propose_one_held_back_change` (agent_recovery/has_diff.rs) skips threads
  whose last turn didn't end `Generated`: interrupted/canceled/failed
  threads recover via Continue, and an Apply button on a failed turn's diff
  would over-claim (same reasoning as ADR 0001). `coding_agent_has_diff`
  keeps the Diff button visible meanwhile.
- **Recovery marks a question-parked thread's pending change `incomplete`
  BEFORE the preserve guard — deliberate, not a bypass.** In
  `recover_orphaned_worktrees` (agent_recovery/recovery.rs), the
  `mark_pending_change_incomplete` re-emit for an actively-running branch runs
  before the `thread_has_unanswered_question` preserve `continue`. That is
  conservative by design: the pending row was populated mid-turn (per-commit
  emits) and the user never confirmed it, so Apply must require explicit
  confirmation whether or not the thread is parked on a question. The
  re-emitted `ChangeProposed` is not in the preserve predicate's terminal
  exclusion list, so the card stays answerable; the flag self-heals to
  `incomplete: false` at the resumed session's next clean idle. Re-flag only
  if the re-emit starts landing an event from the predicate's exclusion list.
- **Trigger threads run one indexed events-table lookup per persisted
  event** (the `is_top_level` latest-start query in
  `event_bus_projection_thread.rs`). Indexed, `LIMIT 1`, accepted cost.
- **The scheduler subscriber receives per-token events before filtering.**
  Broadcast channels deliver to every subscriber; the
  `is_per_token_streaming` blocklist filters after `recv`. Accepted
  semantics — the expensive work (trigger matching) is what's skipped.

- **`startup_permit` releases by drop on every early return** — the spawn
  semaphore permit acquired in `run_direct_agent` is an `OwnedSemaphorePermit`;
  returning the function (spawn failure, input-send failure) drops it and
  frees the slot. "Permit leak on error path" findings are misreading Rust
  drop semantics; the explicit `drop(permit)` at Init is an *early* release,
  not the only one.
- **The 10-minute hung-subprocess watchdog can fire during a long silent
  LLM thinking phase — for BOTH backends, by design.** CC's thinking deltas
  arrive as `stream_event` frames the parser drops; Codex emits nothing
  between items. Either way no AgentEvent flows, `last_event_at` stalls, and
  a >10-min fully-silent gap fires the watchdog — whose action is a
  `ContinuationRequested` auto-resume, not data loss. Flagging this for one
  backend as a new bug requires showing the OTHER signal (tools in flight,
  is_waiting) would have gated CC differently.
- **The Codex driver always closes a turn with exactly one `Result`** — the
  `!turn_terminal_seen` synthesis after child reap covers every "child died
  without `turn.completed`" shape (OOM, auth failure, partial output), and
  the interrupt path synthesizes its own. Findings claiming "no Result
  reaches the engine if drain parsing fails" miss that the synthesis does
  not depend on drain success. The driver tests pin both paths.
- **`coding_agent` fields on ThreadEvents serialize unconditionally — on
  purpose.** Events are append-only and never re-serialized back into the
  store, so there is no "legacy row round-trip" to keep wire-quiet; an
  explicit `"coding_agent":"claude-code"` on new events is self-describing.
  `coding_agent_kind`'s `skip_serializing_if` is not "the pattern" — every
  other `coding_agent` field (TextStreamed, ToolCalled, Idled, …) serializes
  always, and SessionStarted's matches them.
- **`-c model_reasoning_effort=\"high\"` passes literal quotes on purpose** —
  codex's `-c` parses the value portion as TOML, so the quoted form is a
  TOML string (and codex's documented fallback treats unparseable values as
  raw literals, so both forms work). Not a shell-quoting bug: argv is not
  shell-parsed.
- **Codex `ask_user_question` synthesizes a fresh `codex-q-<uuid>` tool_use_id
  per call on purpose** — an MCP server never sees a stable codex-side
  tool-call id, so there is nothing durable to key crash-recovery on. The
  accepted cost (documented at the synthesis site in
  `mcp_permission_server.rs`): a codex child killed mid-question re-asks on
  resume instead of replaying the persisted answer, so the user may answer
  the same question twice across an engine restart. Don't "fix" by hashing
  the question text — a legitimate repeat of the same question later in the
  session would then silently replay a stale answer.
- **Codex drivers synthesize a failed `Result` only for the IN-FLIGHT turn on
  child death; inputs still queued get none** — deliberate parity between
  the exec and app-server drivers. The engine's post-`Exited` path
  (`finalize_direct_agent` + the safety net) settles the thread terminal,
  and the lifecycle docs already accept that merged/abandoned inputs can
  produce fewer Results than inputs. Emitting per-queued-input failures
  would double-fail turns the engine has already classified.
- **`xhigh` and GPT-5.6-scoped `max` ARE valid codex reasoning efforts.** A
  reviewer may flag that `codex_menu_options.json` exceeds the sample config's
  documented minimal/low/medium/high vocabulary. Verified live via app-server
  `model/list`: codex-cli 0.144.4 reports `[low, medium, high, xhigh, max]` for
  GPT-5.6 Sol/Terra/Luna, while GPT-5.5/5.4/5.4-mini stop at `xhigh` (the older
  0.142.5 probe likewise established `xhigh`). The menu carries that exact
  model matrix and `validate_codex_effort` enforces it in both drivers. Re-flag
  only with a reproduced rejection on a current Codex.
  (`runtime/codex.rs`, `runtime/codex_menu_options.json`.)
- **The app-server plan mapper's fresh `plan_<n>` ids do NOT fragment the
  timeline vs the exec tracker's stable item id.** A reviewer may claim exec's
  reused `todo_list` item id renders "one evolving card" while the app-server's
  `plan_seq` ids render one card per revision. Both render identically: every
  `CodingAgentToolCalled` unconditionally pushes a NEW step
  (`exchange-render.ts` `pushStep`); the `tool_use_id` is used only to pair the
  matching result onto the first unresolved step. One step per plan revision is
  also exactly how CC's TodoWrite renders (each call is a fresh tool_use_id).
  Unique ids are strictly safer for result pairing. Re-flag only if step
  grouping starts keying dedup on `tool_use_id`.
  (`runtime/codex_app_server_parse.rs`, `store/thread-events/exchange-render.ts`.)
- **Per-manager `reject_path_traversal` wrappers are deliberate, not DRY drift.**
  A reviewer may flag that `core/apps.rs` and `core/artifacts.rs` each carry a
  private `reject_path_traversal` with the same message, and that
  `commit_data_paths_added/removed` in `core/mod.rs` inline the same check —
  proposing one shared `Result`-returning guard. The consolidation point is the
  *predicate*: `core::is_path_traversal` is the single canonical guard (its doc
  says so), and every site funnels through it. The thin wrappers differ by the
  error type their layer needs (`std::io::Error` for apps, `git2::Error` for the
  git-commit helpers); a shared wrapper would force `map_err` gymnastics at half
  the call sites for a 4-line saving. Re-flag only if a wrapper stops delegating
  to `is_path_traversal` (rule drift), or the message needs to change in
  lockstep and copies have actually diverged.
  (`core/mod.rs`, `core/apps.rs`, `core/artifacts.rs`.)

- **The staged-resource resolvers (`LUCIDOS_CLI_BIN`, `LUCIDOS_SDK_DIR`,
  `LUCIDOS_SYSTEM_KNOWHOW_DIR`) deliberately do NOT share an env-or-fallback
  kernel, and `resolve_system_knowhow_dir` hardcodes the `"system-knowhow"`
  basename.** Reviewers flag both: (1) "third/fourth bespoke
  env-var-else-fallback resolver — extract a shared `resolve_staged_dir`" — but
  the three implement deliberately *different* missing-resource policies (CLI:
  sibling-walk + fail-fast at spawn; SDK: log + serve a stub; system-knowhow:
  warn + None), and whether those policies should converge is the
  separately-tracked silent-degrade-vs-fail-fast question
  (`hardenproj-20260702-sdk-stub-silent-degrade`) — a shared kernel now would
  force-fit divergent contracts. (2) "use `SYSTEM_KNOWHOW_PREFIX` instead of
  the literal" — the `system-knowhow` name is a stable cross-layer contract
  hardcoded equally in shell staging scripts (`RESOURCE_NAMES`,
  `stage_runtime_assemble`), env-pair emitters, docs, and knowhow ids; a
  Rust-side constant covers none of those, so it wouldn't reduce the real
  rename surface. Re-flag only if the policies converge (then extract the
  kernel) or a same-language duplicate pair actually diverges.
  (`core/system_knowhow.rs`, `runtime/lucidos_cli.rs`, `api/sdk.rs`.)

- **The "never leave the thread `running`" settle backstop belongs at the
  CALLER of `run_direct_agent`, not inside it.** Reviewers see per-caller settle
  logic (today `agent_recovery::continue_recovery`, driven from the spawn
  consumer) and propose hoisting it into `run_direct_agent` so every caller
  inherits the floor. It cannot go there: `run_direct_agent` also returns `Err`
  when its spawn guard rejects the call because a **live** session already owns
  the thread (`AGENT_ALREADY_RUNNING_ERROR`). At that point the caller owns
  nothing and the `running` projection is TRUE — it belongs to the turn that won
  the race — so a settle inside the callee would emit a terminal against a
  working session. Only the caller knows whether it owns the turn. For the same
  reason any caller-side backstop must carve that error out (the `Nothing` arm in
  `continue_recovery`); re-flag only if the guard stops returning `Err` for a
  live-session collision. (`agent_session/run_session/run.rs`,
  `agent_recovery/helpers.rs`, `engine_impl/construction.rs`.)

- **`STALE_RESUME_ERROR` is returned to the caller rather than retried inside
  `run_session`, and that is not a missing abstraction.** Three callers each
  implement their own stale-resume retry (chat, the merge/apply Tier-2 arm, the
  spawn consumer's continuation) and it looks like copy-paste begging for a
  shared internal retry. The retry *input* is what differs and it is
  caller-specific: chat re-sends the user's message, the continuation re-sends
  `CONTINUE_RESUME_USER_MESSAGE`, the merge path escalates to a fresh Tier-3
  merge session — each with its own reconstruction/prompt shape. A callee-side
  retry would have to guess. What IS shared is the decision, and that is already
  factored out per caller-family. Re-flag only if two callers' retry inputs
  converge. (`agent_session/run_session/run.rs`, `chat/process_cc.rs`,
  `claude_code/merge_session.rs`.)

- **The packaged updater's `let _ = app.emit(PROGRESS_EVENT, …)` is a deliberate
  best-effort, not a swallowed error.** A reviewer will read the discarded
  `Result` in `updater.rs::emit` as the "no hidden errors" rule being broken. It
  isn't, for two reasons that hold at every one of its call sites: a progress
  frame is *telemetry about* an operation, never the operation, so failing an
  update because a webview didn't receive a percentage would invert the
  priority; and the last two frames (`restarting-services`, `relaunching`)
  deliberately race the client teardown — `app.restart()` is next, and by then
  there may be no webview left to receive anything. Every path that can actually
  fail the update still surfaces a `Failed` frame AND returns `Err` to the
  caller's `catch` (`fail()` does both, precisely so the reason survives whichever
  channel dies first). Re-flag only if `emit` gains a caller where delivery is
  load-bearing rather than narrative.

- **`install_app_update_and_restart` calling `run.commit()` BEFORE inspecting the
  download result is ordering, not an oversight.** It looks backwards — why mark a
  run committed before knowing whether it succeeded? Because winning the commit is
  what proves no cancel took the slot, and that is the precondition for the
  `release()` on the error path being *ours* to make. Checking the result first
  would let a failed download call `release()` after a cancel had already returned
  the slot to `Idle` and a replacement run had claimed it — the exact
  whoever-transitions-to-Idle-owns-it hazard `AppUpdateRun::release` documents.
  `commit()` is a pure state transition with no side effect on the bytes, so
  committing a run that then fails costs nothing. Pinned by
  `a_cancel_that_lands_before_the_commit_wins`.

## Frontend

- **`serverDraft` letting an inbound compose report overwrite a newer PUT ack is
  the accepted, self-healing trade-off — the alternative re-breaks the bug it was
  added for.** A reviewer (Codex flagged this P1) may note that
  `applyRemoteCompose` records every `ThreadComposeChanged` payload as the
  server's current compose state, and that a delayed frame could therefore
  overwrite a newer PUT acknowledgement, briefly making a *superseded draft* look
  clearable when the server actually still holds it. Three reasons it stands:
  (1) The frame is already filtered — our own echo (`origin_device_id`) and any
  in-flight local write (`pendingComposePuts`) are dropped before
  `applyRemoteCompose`, so this needs an SSE frame delayed past a *separate*
  successful PUT round-trip, in conjunction with the user re-typing the exact
  submitted text against a stale watermark. (2) It is transient, not data loss:
  the server still holds the text, so the next thread-summary snapshot re-stages
  it into the composer via `stageDraftFromApi`. (3) The obvious "fix" — refusing
  to let an EMPTY report downgrade a non-empty ack — makes a genuine peer clear
  invisible until the next snapshot, which is exactly the ghost-draft bug
  `docs/plans/2026-07-28-superseded-compose-drafts.md` exists to fix. The
  principled fix is a server-stamped compose version on `ThreadComposeChanged`
  plus the PUT response, so the two reports can be ordered; re-flag with that
  design, not with the ordering suspicion alone.
  (`store/actions/compose.ts` `serverDraft` / `applyRemoteCompose`.)

- **`initiateEngineRestart` dismissing the `engine-new-version` switch toast on
  a spawn-FAILURE path is intentional; the badge is the recovery affordance.** A
  reviewer (Codex flagged this P2) may note that `initiateEngineRestart` dismisses
  the "New version available → Switch to new version" toast up front, and on the
  engine-still-alive failure catches (`ApiError` / Tauri string reject) leaves
  `engineVersionReady` true — so `checkEngineVersion`'s rising edge
  (`ready && !engineVersionReady`) never re-shows the toast, and the switch toast
  is gone until readiness flips again. This is deliberate: (1) recovery is intact
  via the control-panel badge + reload glyph — `engineNewVersionReady()` is
  `engineVersionReady.value` in dev, still true, so the glyph long-press → Restart
  → `initiateEngineRestart` retries (the plan's designated *persistent* affordance;
  the toast is the ready-time convenience). (2) The obvious "fix" — resetting
  `engineVersionReady = false` on failure to force a re-toast — would open the
  client-refresh ordering gate `!(restartRequired || engineVersionReady)` while the
  OLD engine is still running, violating a core invariant of
  `docs/plans/2026-07-01-version-toast-single-surface-and-client-ordering.md`
  (client must never refresh ahead of the engine switch). Dismissing at the top
  (not deferred to after the request) is what gives the instant single surface the
  fix exists for. Re-flag only if the badge/glyph recovery path is removed.
  (`store/actions/chat-changes.ts` `initiateEngineRestart`,
  `store/actions/engine-update.ts` `checkEngineVersion`,
  `components/layout/ControlPanel.tsx` `engineNewVersionReady`.)
- **`removeEventListener(type, fn, { capture: true })` DOES remove a listener
  added with `{ capture: true, passive: true }`.** A reviewer seeing an
  `addEventListener(…, { capture: true, passive: true })` paired with a
  `removeEventListener(…, { capture: true })` may flag a "listener leak" from
  the asymmetric options. It isn't one: per the DOM spec, listener removal
  matches on `(type, callback, capture)` ONLY — the `passive` and `once` flags
  are explicitly NOT part of the match. The minimal `{ capture: true }` on
  removal is the idiomatic, correct form (the navigation focus marker's
  gesture-clear listeners in `components/shared/focusMarker.ts` `applyNavFocus`
  use exactly this). Re-flag only if the `callback` reference or the `capture`
  flag actually differs between add and remove. (`components/shared/focusMarker.ts`.)
- **`.thread-content` (the `tabindex=0` transcript scroll region) deliberately
  shows NO focus outline in any focus state.** A reviewer (Codex flagged this as
  a P2 a11y regression) may note that removing `.thread-content:focus-visible`'s
  outline drops the only visible keyboard-focus cue on the direct-focus paths
  (Tab into the pane, the `⌘⇧2` / Search-Everywhere `focusPaneMainControl`) where
  no per-turn `.nav-focus-stuck` marker is applied. This is a deliberate,
  maintainer-requested visual call: a whole-pane accent ring around the entire
  conversation while arrow-scrolling reads as chrome, not affordance, and the
  keyboard-scroll *function* (Arrow/Page keys, focus landing via `paneFocus.ts`)
  is fully preserved — only the ring is gone. Navigation that lands on a specific
  turn still shows the `.nav-focus-stuck` marker. Re-flag only if the maintainer
  reverses the preference. (`styles/chat/input-messages.css` `.thread-content:focus`,
  `components/layout/paneFocus.ts`.)
- **A deep-link's PULSE element can differ from its SCROLL target — the scroll
  deliberately targets the whole `.chat-exchange`, not the pulsed panel.** A
  reviewer may flag that `scrollToChangeAndPulse` (and `scrollToEventAndPulse`)
  pulse a descendant panel (`.response-panel` on a proposing turn, `.initiator-panel`
  on a resolution card / event) while `scrollToSelectorAndPulse` scrolls (via
  `smoothScrollToElement`) to the `.chat-exchange`, so for an exchange with an
  atypically tall `.initiator-panel` (a user prompt taller than the viewport) the
  pulsed `.response-panel` can land below the fold. This is deliberate: the scroll
  MUST target the `.chat-exchange` because only it carries the `scroll-margin-top`
  gap that keeps the focus border from being clipped under the fixed header /
  sticky title (commit `9884ece31`) — retargeting the scroll to the pulse panel
  would regress that. The pulse-vs-scroll split is inherent to scoping the
  highlight (fixing the original "whole turn highlighted" bug); the below-fold
  case needs the tall-initiator edge AND is degraded feedback, not a broken
  landing (the user still scrolls to the exchange; the sticky border persists on
  the panel). Re-flag only with a concrete fix that keeps the `.chat-exchange`
  scroll-margin behavior intact. (`components/chat/scrollState.ts`.)
- **Turn-nav anchors on the marked turn when present, and only falls back to
  `scrollTop + gap` scroll-position stepping when there's no marker.** Turn-nav
  (`components/chat/scrollState.ts`) lands a turn's top `gap` px below the
  container (the `.chat-exchange scroll-margin-top` clearance). `pickTurnTarget`
  chooses the step mode: when the nav focus marker sits on a listed turn it steps
  by INDEX from that turn (`anchorIdx + direction`); otherwise it steps by scroll
  position via `pickTurnIndex` (comparing `tops[i]` against `scrollTop + gap`). The
  index anchor is what makes turns that share a CLAMPED scroll position reachable —
  after collapsing the last turn, the collapsed turn and an appended "Change
  applied" card cluster in the last (no-scroll-room) viewport, where pure
  scroll-position stepping keys off a pinned `scrollTop` and re-selected the same
  turn (the change card was unreachable — the reported bug). A marker means the
  user has NOT scrolled since the last nav (any scroll gesture fades it), so index
  stepping is unambiguous; the no-marker fallback still handles the
  first-press-from-scroll case and the tested mid-turn "prev snaps to the current
  turn's top" read (both happen precisely when no marker is present), so that
  behavior is preserved. Re-flag only with evidence that a clustered turn is NOT
  reachable via the marker anchor, or that the mid-turn/first-press fallback
  regressed. (`components/chat/scrollState.ts` `stepThreadTurn` / `pickTurnTarget`.)
- **The thread drawer's `id={navKeyDomId(...)}` on rows + section headers does
  NOT violate `frontend.md` "No `id` on dual-rendered components".** The rows
  (`ThreadRowContent`, `ComposingThreadRow`) and section headers carry real DOM
  `id`s because the drawer container is the single keyboard tab stop
  (`role="tree"`, `tabindex=0`) and points `aria-activedescendant` at the active
  row's id — which strictly requires an id, the one thing roving-tabindex would
  avoid but the chosen aria-activedescendant model needs. It's safe because
  `ThreadDrawer` is single-mounted: `App.tsx` mounts only the visible layout's
  pane tree (`mobile ? <MobileSwipeContainer/> : <ThreadDrawer/>`), a breakpoint
  swap is an unmount-then-mount (never two copies coexisting), and within one
  mounted drawer only one view renders at a time and a thread renders once — so
  ids are unique. The rule's hazard (two simultaneous layout copies → wrong-copy
  resolution) is absent. Cross-component lookups still use the `data-` pattern
  (`openHighlightedThreadActions` queries `[data-thread-nav=…]`, like the
  pre-existing highlight scroller), not `getElementById`. Re-flag only if a
  second `ThreadDrawer` can mount concurrently. (`components/drawer/ThreadDrawer.tsx`.)
- **`paneFocus.ts`'s `FOCUSABLE` excluding `[tabindex="-1"]` on EVERY native
  term (not just the trailing `[tabindex]` term) is intentional, not an
  over-broad selector.** A `<button tabindex="-1">` is non-tabbable by native
  Tab, so the per-pane trap must skip it too; the old selector matched it via
  `button:not([disabled])` and let the trap cycle onto it (a latent defect — it
  could land on `aria-hidden` file inputs / the change-actions placeholder).
  This makes the drawer a genuine single tab stop (its mouse-only row buttons are
  `tabindex=-1`) and aligns the trap with native semantics. Re-flag only if a
  control needs to be a trap stop *while* `tabindex=-1` (a contradiction).
  (`components/layout/paneFocus.ts`.)
- **`dispatchForwardedChord` setting `focusedPane = 'content'` for ALL
  forwarded chords — including `toggleThreadDrawer` (⌘⇧1) — does NOT violate
  "the drawer toggle never sets focus" (commit 585274dc3).** That rule governs
  what `toggleThreads` *itself* does (unchanged: plain show/hide, signal-only
  fallback only when closing a drawer that holds focus). The reconciliation is
  a separate fact: a chord forwarded from an app iframe proves the user is in
  the content pane (iframe keydowns only fire when the iframe has focus), and
  iframe pointer events never reach the host's `focusPane('content')` handler,
  so `focusedPane` is otherwise stale. Setting it for *every* forwarded chord
  (not just the three-state toggles that read it) is the more correct design,
  not over-reach: it also fixes `toggleThreads`' close-case housekeeping —
  with a stale `focusedPane === 'drawer'`, closing the drawer from inside the
  app would wrongly bounce focus to `'thread'` (pane.ts line 95); reconciling
  to `'content'` first skips that. Narrowing the set to only the toggles that
  read `focusedPane` would re-introduce the staleness. Re-flagging needs
  evidence that a forwarded chord can originate from an iframe NOT in the
  content pane.
- **`<Overlay>`'s `data-overlay-anchor` marking uses an absolute
  `removeAttribute`, NOT ref-counting like `openOverlayCount`** — and that's
  fine. The asymmetry looks like a leak-in-reverse (two concurrently-open
  overlays sharing one anchor node → the first to close strips the attribute
  while the other still wants it), but no such usage exists: a toggle anchors
  exactly one overlay, and stacked overlays have distinct anchors. The inert
  count is ref-counted because every overlay shares the one `<html>`
  `data-overlay-open` flag; the anchor attr is per-node and per-overlay, so
  set/remove on the captured node is correct. Re-flagging needs a real call
  site where one DOM node is the `anchor` prop of two simultaneously-open
  `<Overlay>`s.
- **`stepThreadPaneWidth` DOES cancel a pending drag snap** — it routes
  through `setSplitRatio`, whose first line is `cancelPendingSnap()`. Only
  `stepThreadDrawerWidth` needs the explicit call because it mutates
  `threadDrawerWidth` directly. The two step paths are symmetric in effect,
  not in spelling.
- **`computeDrawerStepWidth` with a negative/too-small `maxPx` is a no-op,
  not a clamp-to-minimum** — the `if (maxPx < MIN_DRAWER_WIDTH) return null`
  guard runs before any clamping, so an unmounted `.content-row` (offsetWidth
  0) can't lock the drawer at its minimum.
- **`splitRatio` collapse predicates use exact float comparisons on
  purpose** — `=== 0` / `>= 1` are the same predicates `SplitLayout` uses
  for `data-thread-collapsed` / `data-content-collapsed`, and collapsed
  states are only ever *written* as exact `0` / `1` (drag keeps the ratio
  off the edges by 1px; snaps and toggles write the literals). Float-drift
  scenarios ("ratio lands at 0.9999") describe states the layout treats as
  not-collapsed too, so the toggle behavior stays consistent.
- **`e2e/*.spec.ts` use literal localStorage keys** (`lucidos-split-ratio`,
  …) rather than importing `SPLIT_RATIO_KEY` — deliberate: the literal in
  the spec is a canary that the *persisted* contract didn't silently change.
- **The Escape-capture handler cannot starve the keybinding recorder.**
  `handleEscapeCapture` (useKeyboardShortcuts) and the recorder's listener
  (KeyboardShortcutsSection) are both capture-phase listeners on `document`
  — the same node — and `stopPropagation()` does not suppress other
  listeners on the same node (only `stopImmediatePropagation` would). The
  recorder always receives Escape regardless of registration order.
- **`connection.ts`'s `.catch(() => {})` chains are documented
  rejection-tracker silencers**, not swallowed errors —
  `refreshThreadEvents` toasts user-visible failures itself after retrying.
  The justifying comments at the call sites are the carve-out contract
  (`.claude/rules/frontend.md` § best-effort telemetry).
- **The heartbeat's `invoke('heartbeat').catch(() => {})` (useStartup, main.tsx)
  is a local no-op, not a swallowed IPC failure.** Since the tauri 2.11 ACL
  regression, `invoke` itself (`utils/tauri.ts`) records every outcome through
  `utils/ipcHealth`, which writes durable `[Client/ipc]` lines to engine.log —
  first failure immediately, then rate-limited, plus a recovery line. The signal
  is taken at the one chokepoint precisely so call sites need not each report;
  the same holds for the `console.warn`-only handlers in
  `store/actions/native-push.ts`. Re-flag only if `invoke` stops feeding
  `recordIpcOutcome`. (ADR 0028.)
- **`clearStalePendingMessages` bumps inside its mutation guard** — when the
  filter removes nothing, nothing was mutated, so no bump is owed.
- **`appFilters` / `repoFilters` / `triggerFilters` returning `[]` until
  loaded is deliberate** (documented at each site): filter *options* would
  mislabel as "(deleted)" without the registry; this is not Loadable
  masking of displayed data.
- **`threadMap` never evicts threads in-session — by design.** Discarded
  drafts stay with `state='discarded'` (replays would 410 otherwise); page
  reload is the eviction. Consequently `perThread` bump signals and
  `lazyChanges` grow in lockstep with `threadMap` — pruning them alone fixes
  nothing real.
- **`App.tsx` single-mounts the visible pane tree, while header chrome
  still dual-renders** (`ControlPanel` in both `AppHeader` and
  `MobileAppHeader`). Both halves are intentional — don't "fix" either
  direction. The no-`id` rule still binds.
- **`thread.events` is append-only with deduped seqs** — `handleEvent`
  refuses to re-set an existing seq, nothing deletes entries, and wholesale
  rebuilds replace the Map object. The incremental grouping cache
  (`exchange-grouping.ts`) and any future memoization keyed on the Map
  depend on this; see the contract comment at the `thread.events.set` site.
- **The spinner can blink off for `SPINNER_DELAY_MS` at the map-wait →
  events-loading boundary on slow cold starts.** ThreadView's two loading
  phases render `ThreadEmptyState` at different tree positions, so the
  remount restarts `DelayedSpinner`'s delay. Accepted trade-off: carrying
  spinner state across that remount would need module-level timer state for
  a rare, cosmetic path (iOS PWA cold start with slow event loads).
- **`delayedFlagEffect` is deliberately exported** from
  `hooks/useDelayedLoading.ts` even though `useDelayedFlag` is its only
  non-test consumer — it's the fake-timer-testable kernel of the spinner
  delay (this repo has no DOM test rig for hooks). Don't inline it back.
- **`extractLocalFileTarget` excluding only `/data*` and `/apps*` (not
  `/knowhow`, `/artifacts`, `/triggers`, …) is deliberate scope, not a leak.**
  The helper (`utils/linkifyPaths.ts`) classifies a chat anchor href as a real
  local-disk target to hand to the OS opener. It runs LAST in
  `ChatExchange.handleLinkClick` — after the `.artifact-link`/`.app-link`/
  `.nav-link` class handlers and after `extractAppIdFromHref`/
  `extractNavTargetFromHref`. The `/data` and `/apps` guards exist only to catch
  the absolute sub-paths those extractors *decline* (an artifact sub-file like
  `/apps/<id>/styles.css`, or `/data/artifacts/x.pdf` when the artifact rewriter
  didn't run). A bare-absolute workspace path under another prefix
  (`/knowhow/x.md`, `/artifacts/x.pdf`) is NOT a shape the LLM/engine produces —
  they emit `data/`-prefixed or relative paths, and a known artifact is
  rewritten to `.artifact-link` (claimed by the earlier class handler) before
  this fallback is reached. Such an href was ALSO already broken before this
  branch existed (it 404'd against the app origin via the `/data/*`-only static
  mount), so OS-opening it instead changes one dead link into another, not a
  working path into a broken one. Re-flag only with a real producer that emits
  an absolute `/knowhow|/artifacts|/triggers/…` anchor the user is expected to
  click. (`utils/linkifyPaths.ts`, `components/chat/ChatExchange.tsx`.)
- **The snapshot-staleness guards in `thread-loading.ts` (`upsertThread`
  status, `applyEventRows` overlay) keyed on `last_activity` do NOT miss
  status changes whose event omits `last_activity`** (e.g. `ChangeProposed`,
  absent from `LAST_ACTIVITY_EVENTS`). The guard only *suppresses* a snapshot
  that is provably OLDER than already-applied live state
  (`snapshot.lastActivity < meta.updatedAt`); it never has to be the channel
  that delivers a status flip. Live SSE applies every status change via its
  per-event aggregate in `handleEvent` regardless of `last_activity`, and any
  genuinely-later event that makes a refresh "stale" means SSE already holds
  the more-current view. `info.last_activity`, `currentAggregate.lastActivity`
  and `meta.updatedAt` are the SAME monotonic `thread_summaries.last_activity`
  column, so the lexicographic `<` is a valid causal-freshness test — not a
  cross-clock compare.
- **The whole boot-splash stylesheet is in px, not rem, on purpose. It is NOT a
  violation of the "all sizes rem" rule.** The app's `<html>` font-size is scaled
  by `var(--user-ui-scale)` (`base.css`, and 112.5% by default in `mobile.css`),
  while the gateway boot splash is an isolated document at the browser default
  with no access to that scale. One rem value therefore paints at two sizes
  across the cold-boot→workspace seam, which the two documents cross mid-launch
  on the SAME url: at 137.5% scale the mark was 330px in the app and 240px on the
  gateway, and it visibly grew (plus a shifted status line) the moment the app
  document took over. Every length in that block is pinned instead, and both
  sides assert there is no rem in it. The rem rule honors user scale; this is the
  deliberate case where scale must NOT apply. Re-flag only if the gateway splash
  gains UI-scale awareness. (`crates/lucidos-app/index.html`, the block between
  the `lucidos-boot-splash-css` markers.)
- **The gateway splash `include_str!`s `crates/lucidos-app/index.html` and slices
  the stylesheet + mark out of it. That cross-crate reach is the fix, not a
  layering smell.** The splash renders when no engine is reachable, so it cannot
  link a stylesheet; before this it carried a hand-kept copy of every value, and
  the copies drifted (the seam bug above). Sharing the file makes the two
  surfaces one splash by construction. `include_str!` is a build dependency, so
  cargo rebuilds the gateway when index.html changes, and `files_require_restart`
  lists index.html for the same reason. Re-flag only if a real shared-asset
  pipeline appears (a build step that can inline a `.css` file into index.html at
  first paint), which would be the cleaner home for it.
  (`crates/lucidos-gateway/src/proxy.rs` `app_splash_css` / `app_mark_svg`.)

- **Provider-settings components treat a non-`loaded` credentials Loadable as
  "not configured" — sibling-wide pattern, failure surfaced at the section
  level.** `ApiKeyProviderSettings`, `AnthropicProviderSettings`, and
  `LocalProviderSettings` all compute `existing` via
  `status === 'loaded' ? find(...) : undefined`, so a failed credentials fetch
  renders the unconfigured Save flow instead of a per-row error. This reads as a
  "failed must look different from empty" violation, but the credentials-load
  failure is surfaced by `SettingsView`'s `LoadableError` on the credentials
  list, and the three siblings deliberately share one shape. A fix belongs as a
  cross-component change (all three + a section-level gate), not a one-file
  divergence in whichever component a diff happens to touch. Re-flag only as a
  deliberate cross-component cleanup, or if `SettingsView` loses its
  section-level error surface. (`components/settings/ApiKeyProviderSettings.tsx`,
  `AnthropicProviderSettings.tsx`, `LocalProviderSettings.tsx`,
  `SettingsView.tsx`.)
- **Mobile header titles are ABSOLUTELY TRUE-centered on the row middle, with a
  symmetric `max-width` reserve that clamps a long title's left edge past the
  leading icons — this is the explicit product requirement, not the between-icons
  flanking-spacer variant (which was tried and reverted for reading off-center).**
  `.mobile-header-title` (`styles/mobile.css`) is `position:absolute; left:50%;
  transform:translate(-50%,-50%); max-width:calc(100% - 10.5rem)` so it sits on
  the viewport/row axis (like the pane dots + desktop header) regardless of the
  leading (thread-drawer toggle or hamburger + nav, or filter) and trailing
  (actions) cluster widths. The
  requirement was stated as *"they should be centered, as long as they don't
  overlap the left-side icons; if centering would overlap, move them right so the
  left edge clears the rightmost left icon."* The symmetric reserve delivers both:
  a short title reads centered; a long one clamps + ellipsizes with its left edge
  just past the widest leading cluster (~5rem). An in-flow flanking-spacer title
  (commit `14a512b8b`, reverted) satisfied no-overlap but centered BETWEEN the
  clusters, so it drifted off the row middle whenever the clusters differed in
  width — the "Appearance/Threads not centered" report. **Accepted right-side
  residual (do NOT re-flag):** because the reserve is symmetric and the CONTENT
  header's trailing action cluster is variable and can be wide (app/file previews:
  refresh/open/fullscreen/notifications + toggles ≈ 4–5 icons), a very long content
  title's ellipsis tail can pass *visually* under those trailing icons on a narrow
  (375px) viewport — the tail end of the true-center trade-off the user chose over
  the off-center flanking layout. It is tap-safe (see below) and the alternatives
  are worse (off-center flanking was rejected; a reserve wide enough to clear 5
  icons would truncate common content titles to ~60px). Overlap handling: the
  title box is `pointer-events:none` (taps fall through; brand's visible children +
  the content title re-enable them), and `.mobile-header-row .content-header-actions`
  is `z-index`-lifted so a long CONTENT title's ellipsis tail can't intercept an
  action (only shows through behind it). The brand's long **workspace name** can't
  spill over the left icons because `.pane-header-brand-label` is bounded to the
  centered box (`max-width:100%`) so `.workspace-name-label` ellipsis-truncates
  within it. That truncation — NOT the name-hide budget — is the no-spill
  guarantee, which is why `ConnectionStatus.tsx` MUST keep summing the trailing
  `.pane-header-spacer` width into `available`: the absolutely-centered mobile
  brand is shrink-to-content, so `brandLabel.clientWidth` has no slack past the
  text, and the spacer is the room the name occupies. Dropping that spacer term
  collapsed the budget to the text width and latched the name hidden — the
  "workspace name gone from the brand" regression. (Desktop has no spacer siblings
  and a fixed-width brand-label with its own slack, so the sum is a no-op there.)
  Guarded by
  `e2e/mobile-threads-title-alignment.spec.ts` (center ≈ row middle + left edge ≥
  leading cluster). Re-flag only if the title loses `position:absolute` (regresses
  to off-center) or the content actions lose their `z-index` lift. The content
  title's tap tooltip (`e2e/tooltip-swipe-dismiss-mobile.spec.ts`) works because
  `.mobile-content-title` re-enables `pointer-events`.
  (`components/layout/MobileAppHeader.tsx`, `components/layout/ConnectionStatus.tsx`,
  `styles/mobile.css` `.mobile-header-title` / `.mobile-content-title`.)

- **A few CSS custom properties are referenced but never defined — that is the
  point; they carry a `var()` fallback.** A scan for "undefined custom property"
  flags `--border-subtle` (`chat/response.css` `.resume-note-body`, `steps.css`)
  and `--accent-contrast` (`pages.css`) as broken references. They are not: every
  use passes a second argument — `var(--border-subtle, var(--border-color))`,
  `var(--border-subtle, var(--bg-tertiary))`, `var(--accent-contrast, #fff)` — so
  the property is an OPTIONAL override hook with a working default, and CSS
  resolves it to the fallback when the token is absent. Two more that look
  undefined for different reasons: `--toast-accent` is declared per-type inline
  (`.toast-warning { --toast-accent: … }`), so a line-anchored `^\s*--x:` grep
  misses it, and `--user-ui-scale` / `--app-height` / `--thread-depth` are set
  from JS at runtime. Re-flag only a `var(--x)` with NO second argument whose
  token is absent from every `:root`/theme block and never assigned from JS.

- **`path_is_in_cc_worktree` deliberately exists in THREE copies (bash, gateway,
  engine) — do not "extract the duplicate".** A reviewer will flag the same 8-line
  predicate in `scripts/lib/workspace.sh`, `crates/lucidos-gateway/src/stack.rs`, and
  `crates/lucidos-engine/src/paths.rs` as a DRY violation. There is no shared home: one
  copy is bash, and the gateway crate has NO dependency on the engine crate (check
  `crates/lucidos-gateway/Cargo.toml` — the two are independent binaries by design), so
  sharing would mean adding a workspace member crate for one function. Each copy carries a
  doc comment naming the other two. Re-flag only if a shared crate already exists for
  another reason, or if the three implementations have actually diverged in behaviour —
  they are pinned by unit tests on each side (`stack::tests::detects_coding_agent_worktree_paths`,
  `paths::tests::flags_coding_agent_worktree_paths`, and
  `workspace_test.sh::test_worktree_predicate_classifies_paths`). See ADR 0021.

- **The System page's "Client" row shows a build id, not a version — and the engine's
  CalVer must NOT be baked into the bundle to "fix" that.** A reviewer will see
  `Client 1ba1c823d933` next to `Engine 2026.07.27.1` and read it as an unfinished
  row, or will find that `crates/lucidos-app/vite.config.ts` once had an
  `engineVersionPlugin` (removed) and conclude this reverted the `addWatchFile` fix in
  e337cc980. Neither holds. The web client has no version of its own: the only thing
  identifying it is `CLIENT_BUILD_ID`, which is also the exact value the refresh badge
  compares against the served `sw.js` build id, so the row and the badge agree by
  construction. Baking the engine's VERSION in instead produced a value frozen at
  bundle-build time that drifted on every engine-only Apply (nothing rebuilds the
  frontend when only `VERSION` changes), showing two disagreeing numbers no reload
  could reconcile. Re-baking on each bump is worse, not better: every engine-only
  change would then emit a byte-different bundle → new `sw.js` BUILD_ID → a "refresh
  to sync" toast whose whole payload is a version string, destroying the property that
  a pure engine-only *Switch* surfaces nothing (`store/actions/connection.ts`). And
  e337cc980's `addWatchFile` fed `resolveClientVersion`'s "(latest: X)" comparison,
  which was itself deleted later as a phantom-update source — so the plugin had no
  remaining consumer but this one display row. Tauri keeps a real Client version (a
  versioned shell with a real updater). Pinned by
  `components/settings/clientVersionSource.test.ts`; re-flag only if the web client
  gains a genuine version of its own.

## Scripts (bash)

- **`record_instance_port`'s `2>/dev/null || true` is deliberate, even though
  the marker it writes is load-bearing.** `install.sh`'s helper carries a long
  comment saying the `<data>/port` marker is the whole of instance discovery
  (no marker means invisible to `uninstall.sh --list` and unremovable by
  `--all --purge`), and then tolerates its own write failing, which reads like
  the silent-degrade the comment argues against. Three reasons it stays. Both
  call sites write it immediately after a `mkdir -p "$data"` that is itself
  `|| die`, so the directory provably exists and is writable a line earlier;
  the only remaining failure is a filesystem going read-only mid-install, which
  the very next step (`exec`ing the gateway, or health-checking the service)
  fails on anyway. The tolerance is inherited from `register_service`, where it
  is load-bearing in the other direction: a marker write must never abort an
  otherwise-successful registration. And the observable consequence is now
  asserted end to end rather than assumed, by `install-smoke.yml`'s front-door
  rungs 5-8 (`--list` must show the instance; `--all --purge` must remove the
  data dir and the shared runtime) and by `service_test.sh`'s marker +
  `service_list_instance_names` assertions on both foreground paths. Re-flag
  only with evidence of a real path where the write fails while the install
  otherwise succeeds. (`install.sh` `record_instance_port`.)

- **The em-dash hook's four `jq` calls must NOT be short-circuited by grepping
  the raw payload first.** `.claude/hooks/no-em-dashes.sh` spawns `jq` up to
  four times per `Edit` (tool_name, file_path, old_string, new_string), and the
  obvious optimization is to `grep -qF` the banned characters over the raw stdin
  and `exit 0` when absent, since a field cannot carry what the payload does not.
  It is not safe as written: JSON may encode the character as a `\uXXXX` escape
  rather than as raw UTF-8 bytes (`JSON.stringify` does not escape non-ASCII,
  but nothing in the hook contract promises it never will), and a raw-bytes grep
  would then wave the write straight through, silently disarming the primary
  gate. Decoding is exactly what the `jq` calls are for. A fast path would have
  to match the escape forms too, and the few milliseconds saved per edit do not
  pay for that coupling. Re-flag only with evidence that the payload encoding is
  contractually raw UTF-8. (`.claude/hooks/no-em-dashes.sh`.)

- **`dev-runtime.md` / `build-release.md` list scripts in `paths:` that their
  bodies never name — that over-match is deliberate.** Reviewers flag that
  `dev-runtime.md` matches `scripts/lib/{sleep,preflight,host_load_guard*,webkit_reaper*}.sh`
  and `scripts/lib/{sigterm_contract_test,wait_for_engine_shutdown_test}.sh`
  while its prose never mentions them — so a session editing one of those loads
  ~9.5k of rules that don't describe it. That is the chosen trade. The two rules
  are the ONLY ones matching anything under `scripts/`, so a path dropped from
  both gets **no rule at all**, and a silently-absent rule is indistinguishable
  from a rule that doesn't exist (the exact risk the split was warned about).
  Those libs are dev/e2e process infrastructure — supervisors, teardown
  contracts, host-load and WebKit reaping — living in the same lifecycle world
  the file's gateway/e2e sections describe, so the loose match is the safer
  side of the trade. Also do NOT re-derive "the lists should be `scripts/**`":
  a catch-all on either file defeats the split, since a build-script edit would
  again pull all 58.6k. Re-flag only if a third rule starts covering `scripts/`,
  or if one of those libs grows a home in another rule file.
  (`.claude/rules/dev-runtime.md`, `.claude/rules/build-release.md`, CLAUDE.md
  § rules index.)

- **Under `set -e`, a short-circuited `[ cond ] && action` does NOT exit the
  script — but `x="$(cmd)"` DOES.** These two look equally innocent and are not.
  Bash exempts "any command executed in a `&&`/`||` list except the command
  following the final `&&`", so when `[ "$rc" -ne 0 ] && overall_rc=$rc` takes
  the false branch the list returns 1 and execution continues — the idiom is
  safe at the end of a loop body, an `if` branch, or a top-level line (verified
  empirically; it is used throughout `scripts/lib/e2e.sh` and
  `scripts/e2e-browser.sh`). It is unsafe only as the LAST statement of a
  *function*, where the 1 becomes the function's return value. By contrast a
  bare assignment from a command substitution takes the substitution's exit
  status and IS subject to errexit: `set -e; x="$(exit 3)"` exits 3 on the spot.
  So a helper whose last command can fail (a `find`, a `grep`, an `awk`) must
  end `|| true` before its output is captured that way — see
  `_first_build_input_newer_than` in `scripts/lib/e2e.sh`, where a transient
  `find` error would otherwise abort the whole e2e run rather than just
  rebuilding. Flag the assignment form; don't flag the `&&` form.

- **`VAR=x some_function` does NOT leave `VAR` set after the function returns —
  so a test that asserts on `$VAR` afterwards can only ever pass.** A variable
  assignment prefixed to a *function* call is scoped to that call in bash's
  default (non-POSIX) mode and restored on return, unlike the same prefix on an
  external command. `HOST_LOAD_SAMPLER_PID="" HOST_LOAD_GUARD_DISABLE=1
  start_host_load_sampler` followed by `[ -n "$HOST_LOAD_SAMPLER_PID" ]` therefore
  reads empty even when the function really did spawn a sampler and assign the
  pid — a vacuous assertion that reports "not started" for both outcomes
  (verified empirically on bash 3.2). Assert on a side effect the function writes
  outside its own scope instead — a pidfile, a marker, a log line. Flag this shape
  in tests; the production call sites are unaffected because they read the
  variable *inside* the same call. (`scripts/lib/host_load_guard_test.sh`
  § `test_sampler_disabled_with_the_guard`.)

- **A single-quoted `trap` body that contains a double-quoted command path
  runs the command correctly — the quotes are not literal.**
  `trap '"$BIN/pg_ctl" -D "$DATA" -m fast stop || true' EXIT` registers the
  literal string at trap-set time (single quotes), then re-parses it as a
  shell command at fire time — at which point the inner double quotes are
  ordinary quoting (removed during parsing) and `$BIN`/`$DATA` expand. It does
  NOT try to exec a command literally named `"$BIN/pg_ctl"`. Verified
  empirically: the trap invokes the real binary with correctly expanded,
  space-safe argv. (`scripts/prototype/desktop-pg-pgvector-spike.sh`.)

- **`curl … | sh` pipes only STDIN; STDOUT stays the terminal.** In a
  `curl -fsSL …/install.sh | sh` invocation the pipe connects curl's stdout to
  the shell's stdin (fd 0); the shell's stdout (fd 1) and stderr are inherited
  from the controlling terminal. So a child script's `[ -t 1 ]` (stdout-is-a-tty)
  test is TRUE under the documented one-liner — e.g. `install.sh` runs
  `scripts/web-dev.sh`, whose `elif [ -t 1 ]` branch prints the listening line
  and returns (it does NOT take the blocking `wait`-on-supervisor branch), so
  the installer's success banner is reached. A reviewer reasoning "piped, so no
  tty → it hangs" has conflated stdin with stdout. The only invocation that
  makes fd 1 a non-tty is an explicit redirect (`curl … | sh > file`), which is
  not the documented path. (`install.sh`, `scripts/web-dev.sh` tail.)

## CI workflows

- **`front-door`'s payload gate on `push: rc/**` racing the RC publication is
  the chosen design, not an oversight.** Reviewers flag that the job fetches
  `https://rc.lucidos.dev/install.sh` the moment the rc branch is pushed, while
  the RC copy is published by a separate, asynchronous step — so an ordering
  skew reds a perfectly good release candidate. The mechanism is real; the
  conclusion isn't. Three reasons it stays: (1) the race is inherent to the
  requirement "check the RC payloads on every `rc/**` push", not to any one
  assertion — with the baked-version check removed, an unpublished route still
  soft-404s and rung 1 still reds at the HTML sniff; (2) fail-closed is the only
  correct posture for a gate, since a gate that cannot see its artifact must not
  pass, and the diagnosis names the likely cause verbatim ("the site publisher
  has not published this RC's installer yet"); (3) the payload legs are
  independent of the rest — they red alone, and `smoke` / `dmg-verify` /
  `tarball-smoke` are unaffected. (Since `front-door-macos` was added there are
  three payload legs — `front-door` plus its two macOS matrix legs — reading the
  *same* origin, so an unpublished RC reds all three at once. That is one
  failure reported three times, not three independent findings.) The
  "trigger after publication" alternative IS what the *production* front door
  does (a publisher-fired `workflow_dispatch` after `SitePublished`); the RC
  gate deliberately runs earlier so nothing reaches the real path unchecked.
  Re-flag only with new evidence that the publisher cannot publish before the
  rc push — in which case the fix is a bounded poll for the matching baked
  version, NOT dropping the assertion.
  (`.github/workflows/install-smoke.yml` § front-door, `scripts/release.sh`
  § `print_rc_gate_handoff`.)

- **`front-door-parity`'s asymmetric severity is deliberate, and so is its
  checkout.** Two things about that job read as inconsistencies and are not.
  (1) *Production serves a route the candidate does not* is FATAL while the
  reverse is only a WARNING. Reviewers propose symmetry. It would be wrong: an
  in-flight candidate that ADDS a publish route legitimately leads production
  until publish, so a symmetric gate reds the daily cron through every such
  release window. The direction is not left uncovered either, it is covered
  later and harder: once the GA publishes, production's own served installer
  declares the route and `front-door` rung 1 reds fatally on it. Likewise
  "missing at both" is a warning because that is `front-door`'s verdict to give,
  in the same daily run. (2) It runs `actions/checkout` while `front-door`
  pointedly does not. The no-checkout rule protects the *subject* of the test:
  front-door's whole input must be what the origin serves. Parity's subject is
  still the two origins, and `front_door_parity.sh` derives every route set from
  the served scripts; the tree is only the measuring instrument, which is what
  buys ShellCheck coverage and a hermetic test. Re-flag only if the harness
  starts reading routes from the checkout.
  (`.github/workflows/install-smoke.yml` § front-door-parity,
  `scripts/lib/front_door_parity.sh`.)

- **`release-tarballs.yml`'s delete-then-upload attach loop cannot clobber a
  signed macOS tarball.** Reviewers read the automatic `release: published`
  attach — which runs for all four matrix entries, macOS included, and deletes
  any same-named asset before uploading — next to the claim that the signed
  macOS tarball comes from `build-dmg.sh --emit-tarball`, and conclude that CI
  overwrites a signed asset with an unsigned one. It cannot: **nothing else
  ever attaches a headless tarball.** `build-dmg.sh`'s only `gh release upload`
  sends exactly the DMG + `Lucidos.app.tar.gz` + `.sig` + `latest.json`, and
  `release.sh` never passes `--emit-tarball` — so `--emit-tarball` is a
  capability no release flow invokes, and every `lucidos-<ver>-<triple>.tar.gz`
  on a Release came from this workflow. (Confirm on any release by asset
  timestamps: on v0.17.0 the DMG trio landed at 04:08:53 and all eight headless
  assets at 04:19–04:51.) The macOS tarballs a user downloads are therefore
  unsigned, which is accepted rather than accidental — a `curl`-fetched file
  carries no `com.apple.quarantine` xattr so Gatekeeper never assesses the
  runtime (ADR 0027's reasoning), and `install.sh`'s `verify_runtime_executes`
  runs the gateway once at install time so a refusal is loud and immediate.
  Re-flag only if a release path starts attaching a signed headless tarball
  (then the clobber ordering becomes real), or if macOS begins quarantining
  curl-fetched files.
  (`.github/workflows/release-tarballs.yml` § attach step, `scripts/build-dmg.sh`
  `gh release upload`, `.claude/rules/build-release.md` § Linux tarballs via CI.)

- **`front-door` and `front-door-macos` duplicate their step bodies ON PURPOSE,
  and the asset preflight is duplicated with them.** A reviewer meets two nearly
  identical validate steps, two payload sniffs, two ~80-line preflights and two
  health polls, and proposes a composite action, a `needs:` chain, or a shared
  `scripts/lib` helper. All three break something load-bearing. The jobs must
  report **independently**: a Linux-only outage must not hide the Mac verdict or
  the reverse, which a shared job or a `needs:` edge would do. `front-door` must
  keep **no checkout**, since its entire input has to be what the origin serves,
  so a `scripts/lib` helper is unavailable to it by construction (that rule is
  what `front-door-parity` is explicitly carved out of, and it pays for the carve
  by not being a front-door job). And a composite action is a third dialect on
  top of two host families that genuinely differ, in the launch shape (`launchd`
  exits 0; the container holds the foreground) and in bash version (Apple ships
  3.2, hence `${FD_HDR[@]+"${FD_HDR[@]}"}` on the macOS side only). The real
  hazard of duplication, silent divergence, is answered with a test rather than
  a refactor: `scripts/lib/front_door_gate_test.sh` runs every invariant **once
  per job**, so a fix landing in one job reds. Re-flag only with evidence the two
  have diverged in a way that test does not cover, and then extend the test.
  (`.github/workflows/install-smoke.yml` § front-door + front-door-macos,
  `scripts/lib/front_door_gate_test.sh`.)

- **`front_door_gate_test.sh`'s file-wide `# shellcheck disable=SC2016` is the
  correct scope, not a blanket silencer.** Every needle in that file is LITERAL
  shell text to find inside the workflow (`"$INSTALL_PID"`, `$tarball_url`,
  `$RUNNER_TEMP/front-door-payloads`), so the `$` must reach `grep` unexpanded.
  Expanding any of them, all unset in the test's own shell, would turn the needle
  into the empty string and the assertion into a vacuous pass, which is the one
  failure a drift test cannot afford. Per-line disables were considered and are
  worse here: nine of them across the file, each restating the same reason. Note
  that even the `FASTFAIL=...` assignment trips SC2016, so hoisting needles into
  variables does not avoid it. Re-flag only if the file grows a use of single
  quotes where expansion WAS intended.
  (`scripts/lib/front_door_gate_test.sh` header.)

## Plugins & triggers (ADR 0019)

- **The `trigger.toml` projection is gitignored-by-`.git/info/exclude`, and that
  degrades gracefully (to untracked files) when `.git` is a worktree *file*
  rather than a dir.** `ensure_trigger_toml_gitignored` (`triggers/definition.rs`)
  writes `<ws>/.git/info/exclude`; in a git worktree `.git` is a file, so the
  write silently no-ops and the projected `trigger.toml` files show as untracked.
  This is INTENDED and acceptable: real Lucidos *workspaces* are normal repos
  (`.git` is a dir), and the files are a derived read-model regenerated from
  events anyway — untracked is harmless. Re-flag only if workspaces start being
  provisioned as worktrees. (`triggers/definition.rs`.)

- **The boot rebuild rewrites every `trigger.toml` unconditionally (no
  content/mtime compare).** `rebuild_trigger_definitions` runs once at boot in
  `spawn_blocking` over a handful of triggers; the writes are deterministic
  (identical bytes when unchanged) and the files are gitignored, so there's no
  git churn and the cost is negligible. Not worth a content-equality
  short-circuit. (`triggers/definition.rs`, `scheduler/mod.rs` boot path.)

- **`appSearchOpen` / `appSearchQuery` are deliberately shared by the Apps and
  Plugins panels.** Only one of the two panels is visible at a time, so one
  search-state pair serves both (the store comment says so). A leftover query
  can carry across a panel switch — this is the same accepted behavior the old
  Installed/Store tabs had, not a new correctness break. Re-flag only if the
  panels become simultaneously visible. (`store/store.ts`, `store/actions/apps.ts`.)

- **`get_recent_threads` dropping the `coding_agent_proposed` out-of-window
  bypass does NOT lose work behind the archive curtain.** The outer `WHERE` only
  returns inbox rows + the contiguous archived window — it no longer force-loads
  an archived `coding_agent_proposed` row. This looks like it violates the
  "archived-with-pending-changes routes to Current" invariant
  (`display_section`, test `archived_with_pending_changes_routes_to_current`), but
  archived + `coding_agent_proposed=TRUE` is an UNREACHABLE state: `ChangeProposed`
  (the event that sets `coding_agent_proposed`) and `CodingAgentIdled` both
  transition the thread `to_inbox` (`thread_lifecycle.rs` transition table), so a
  proposed CC thread is always inbox; `is_blocking` removes the Archive action
  while an in-workspace change is pending; and the external-repo archive cascade
  emits `ChangeApplied` (clearing proposed) before `ThreadArchived`. The
  `display_section` arm is a defensive property, not proof of reachability.
  Re-flag only if a path is added that sets `coding_agent_proposed` WITHOUT a
  `ChangeProposed`/`to_inbox` transition. (`core/store/threads/summaries.rs`,
  `engine/thread_lifecycle.rs`.)

- **`base.css` is NOT served to app iframes — host-only theme tokens belong in its
  theme blocks.** `/api/v1/sdk-iframe.css` is exactly two `include_str!` pieces
  (`crates/lucidos-engine/src/api/sdk.rs`): the engine's own `sdk_iframe.css`
  (which carries its OWN "keep in sync with base.css" token mirror) plus
  `shared-components.css`. A reviewer may flag a new host-chrome token in
  `base.css`'s `html`/`html[data-theme]` blocks (e.g. `--focus-pill-*`) as
  "ships to every app iframe" or "belongs in `host-components.css`" — both wrong:
  base.css never reaches apps, and `.claude/rules/frontend.md` explicitly keeps
  ":root/theme token blocks" in `base.css` (`host-components.css` is for component
  *rules*, not tokens; `--header-gradient`/`--titlebar-strip-bg` are the
  host-chrome-token precedent). The js-sdk.md "Theme variables" drift rule applies
  only to tokens added to `sdk_iframe.css`/`shared-components.css` themselves.
  Re-flag only if the token is added to one of those two served files without the
  doc row. (`styles/global/base.css`, `crates/lucidos-engine/src/api/sdk.rs`.)

- **`openScaleModal` deliberately does NOT blur the UI-scale trigger button, and
  the trigger uses plain `.settings-option`.** A reviewer (Codex did) may flag
  that the value button stays `:focus`'d under the full-screen scale modal, so a
  focus ring "could" show behind the dim backdrop on WebKit/iOS, and propose
  blurring it on open. This was actually tried (a `document.activeElement.blur()`
  in `openScaleModal`, plus a `.settings-value-button` class overriding the
  outline) and **reverted at the user's explicit request** — the retained focus
  ring behind the modal is original, long-standing behavior AND, empirically, is
  NOT the "weird halo" users report: that halo is the *mobile slider thumb* (the
  old `background-clip:padding-box` + transparent-border trick shrank the visible
  dot to 1.25em and let iOS's default thumb bleed through; fixed by the solid
  2.5em thumb in `toggle.css`). iOS also does not persist a focus ring on a
  `<button>` after a tap. Re-flag only with evidence the *value button's* ring is
  actually visible behind the modal AND is what a user reported. (`components/
  shared/scaleModalState.ts`, `styles/settings/toggle.css`.)

- **`thread_has_unactuated_continuation` defaults to `false` (= emit anyway) on a
  DB error — deliberately.** Reviewers flag that a transient DB failure on this
  guard query lets `resume_pending_switches` emit a fresh `ContinuationRequested`
  while the startup orphan re-dispatch separately drives an older unactuated one —
  two event ids for one thread past the per-EVENT idempotency set. That trade-off
  is chosen: the alternative (skip the emit on error) can silently strand a
  user's auto-resume, which is the exact zombie bug the mechanism exists to fix.
  The double-actuation corner needs a transient DB failure precisely on the guard
  query AND a scan-vs-commit timing race, and is bounded downstream by the
  per-thread spawn coalescer (`cc_spawn_coalesce.rs`) and `run_direct_agent`'s
  single-lock "already running for this thread" guard — worst case is one refused
  duplicate spawn attempt, never two live `--resume` subprocesses. Re-flag only if
  one of those downstream guards is removed or the default flips.
  (`crates/lucidos-engine/src/engine/agent_session/spawn_dispatcher.rs`,
  `crates/lucidos-engine/src/engine/engine_version.rs`.)

- **The Notifications "Unread" tab has no inline `failed`-state error surface —
  deliberately.** Reviewers flag that `NotificationsView` renders
  `unreadNotifications` for the "Unread" tab, and `loadUnreadNotifications` never
  sets `'loading'` or `'failed'` (it applies in place so the bell badge never
  blinks to 0 on a reload), so a *total* cold-start outage — every unread load
  failing from app-start through panel-open — leaves the Unread tab on a
  delay-gated skeleton forever instead of a `<LoadableError>`. That is chosen:
  the unread set is a best-effort poll (`.claude/rules/frontend.md` §
  "Carve-out: best-effort telemetry") whose failure IS surfaced — the debounced
  connection dot and the "Unread count is stale — couldn't reach the engine after
  3 tries" toast (`unreadLoadFailures`) — just not inline in the tab. A skeleton
  is the least-bad inline option: a blank panel looks broken, and rendering "No
  unread notifications" on an outage would be a FALSE empty (violates "failed must
  look different from empty"). Any single successful load heals it permanently,
  and reconnect / resume / any SSE all retry. The single-source design (Unread tab
  == badge source) is the whole point — routing the tab back through the
  `failed`-capable `notifications` browse fetch would reintroduce the badge/list
  drift this fixed. Re-flag only if the badge stops deriving from
  `unreadNotifications`, or if `loadUnreadNotifications` gains a surfaced failure
  state that the view could read without blinking the badge.
  (`crates/lucidos-app/src/components/notifications/NotificationsView.tsx`,
  `crates/lucidos-app/src/store/actions/notifications.ts`.)

- **Only the `data/` writable root is width-checked; the git-common-dir root is
  not — deliberately.** Reviewers notice the asymmetry in
  `codex::sandbox_writable_roots`: the resolved `data/` path goes through
  `widens_past_the_workspace` (which refuses `data -> .` / `data -> /`, so a
  symlink can relocate the sandbox hole but never widen it), while the git
  common dir is pushed straight on. The guard is absent there because the case
  is unreachable, not because it was forgotten: the git root comes from `git
  rev-parse --git-common-dir`, which always answers with a `.git` directory
  belonging to the worktree's own repository. For it to contain the workspace,
  the workspace would have to live *inside* `<repo>/.git/` — not a layout the
  engine can produce. Adding a guard for it would be unreachable defensive code
  on a security-sensitive path, where an unreachable branch is strictly worse
  than none (it reads as a real case and invites someone to "fix" it). Both
  roots ARE canonicalized, which is the property the seatbelt actually needs —
  that one is load-bearing and covered by
  `the_git_root_is_canonicalized_too_not_just_the_data_root`. Re-flag only if
  the git root ever starts coming from somewhere other than git's own
  resolution (e.g. a config-supplied path).
  (`crates/lucidos-engine/src/runtime/codex.rs`.)

- **`TriggerRunHistory.created_at` is DB-clock on purpose, and its
  fail-open case is accepted.** A reviewer who has just read the clock-skew
  fix in `triggers/run_history.rs` will notice that `last_run` is carefully
  engine-clock while `created_at` is plain `events.created`, and flag the
  `SlotPredatesTrigger` check as failing open under the very skew the fix
  addresses. That is correct and known: `TriggerCreated` carries no
  engine-clock timestamp in its payload, so there is nothing else to read.
  The consequence is bounded — a brand-new trigger fires *once* for a slot it
  never existed for, which is exactly the behavior that predates the check —
  and it can never produce a double-fire, because that is guarded by
  `last_run`. Making it exact means adding an engine-clock field to the
  `TriggerCreated` payload, which would only help triggers created after the
  change. Re-flag only with a proposal that also covers legacy rows, or if
  `last_run` ever starts feeding on a DB-clock value again.
  (`crates/lucidos-engine/src/triggers/run_history.rs`.)

- **The no-Lucidos-source spawn guard does NOT break cross-workspace
  `run_coding_agent` — the forwarding path returns before it.** Reviewers see
  `if folder_input.is_none() && !crate::paths::has_lucidos_source()` in
  `agentic_loop_special_tool.rs` and flag that a packaged install can no longer
  route a folder-less spawn to a `workspace="dev"` target that *does* have a
  checkout. It can: the `workspace_arg` branch a few lines above returns
  `cross_workspace_run_coding_agent(...)` outright, so a cross-workspace call
  never reaches the guard, and the receiving engine applies its own check via
  `run_session`'s `unregistered_lucidos_root`. The guard is local-spawn-only by
  position, not by an explicit `workspace.is_none()` term — which is what makes
  it read as unguarded. (Codex flagged exactly this on 2026-07-29; the *prompt*
  wording was genuinely over-broad and was scoped, but the code was correct.)
  Re-flag only if the cross-workspace early return moves below the guard.
  (`crates/lucidos-engine/src/engine/agentic_loop_special_tool.rs`,
  `crates/lucidos-engine/src/engine/agent_session/run_session/run.rs`.)

- **A blank `client_secret` on an `oauth_client` credential is a deliberate
  choice, not a missing validation.** Reviewers see `prepare_oauth_flow` and
  `refresh_oauth_if_needed` accept an absent/blank `client_secret` where they
  used to `ok_or("Missing client_secret")`, and the modal no longer marking the
  field `required`, and flag it as a dropped guard that will produce a broken
  half-configured credential. It is the feature: `ClientAuth::from_secret`
  reads a blank secret as OAuth's *public client* (RFC 8252) and authenticates
  the redemption with PKCE (`S256`) instead — the correct shape for a desktop
  app, and the only way a Microsoft Entra "Mobile and desktop applications"
  registration can be redeemed at all (a secret there fails with
  `AADSTS90023`). The confidential path is unchanged and pinned by snapshot
  tests over the authorize query params and the exchange form pairs. Re-flag
  only if a code path starts sending PKCE *and* a secret together, or omits
  the secret on the refresh leg while sending it on the exchange leg (the two
  legs must agree on client type).
  (`crates/lucidos-engine/src/core/oauth.rs`,
  `crates/lucidos-app/src/components/credentials/CredentialModal.tsx`.)

- **`CALLBACK_HOSTS` accepts `[::1]` even though the IPv6 bind is
  best-effort.** `resolve_redirect_uri` runs before `CallbackListener::bind`,
  so it cannot know whether `::1` actually bound, and a reviewer will flag the
  window where an IPv6 override is accepted on a host that then falls back to
  IPv4-only. Accepted: no provider is known to require the IPv6 literal (it is
  documented as the last-resort form), the fallback logs
  `IPv6 loopback [::1]:… unavailable`, and the flow still fails inside the
  existing 120s timeout rather than silently succeeding wrong. Re-flag with a
  provider that actually needs `[::1]`, or if the bind result becomes available
  before the resolve.
  (`crates/lucidos-engine/src/core/oauth.rs`.)

## Settled architecture questions

- **No shared turn-lifecycle orchestrator across the agent-session loop and
  the chat agentic loop** — ADR 0003. The seam already exists
  (`lifecycle.rs` pure decision functions + the typed terminal helpers).
- **External-repo coding-agent threads stay out of the change/dot/blocking
  machinery** — ADR 0001.
