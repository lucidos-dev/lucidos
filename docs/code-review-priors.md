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

## Frontend

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

## Scripts (bash)

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

## Settled architecture questions

- **No shared turn-lifecycle orchestrator across the agent-session loop and
  the chat agentic loop** — ADR 0003. The seam already exists
  (`lifecycle.rs` pure decision functions + the typed terminal helpers).
- **External-repo coding-agent threads stay out of the change/dot/blocking
  machinery** — ADR 0001.
