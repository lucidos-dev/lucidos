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

- **The API-drop auto-resume cannot reset its own budget, so it cannot loop
  forever.** `api_error_auto_resumes_spent` (`agent_session/resume.rs`) counts
  `ContinuationRequested{auto_resume_after_api_error}` newer than the thread's
  last `MessageReceived` or `ResponseGenerated`, and a reviewer reasonably
  worries that the resume it authorizes injects
  `CONTINUE_RESUME_USER_MESSAGE` into the subprocess and so lands a
  `MessageReceived` that zeroes the count on every pass. It does not: the
  continuation is delivered on the spawn path, not the chat path, and emits
  `ContinuationRequested` → `ContinuationStarted` → `SessionStarted` with no
  `MessageReceived` anywhere (verified against a live
  `auto_recovery_after_hang` sequence, which uses the identical dispatch).
  Consecutive drops therefore accumulate to `MAX_API_ERROR_AUTO_RESUMES` and
  stop. Re-flag only if the continuation dispatch starts emitting a
  `MessageReceived` for its injected text.

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

- **Command text surfaced to the user is scrubbed by `redact_postgres_secrets`
  and nothing more, on purpose.** Reviewers see a command embedded in a
  persisted / pushed string and ask for broader credential sanitisation (a
  `curl -H 'Authorization: Bearer …'` would ride through). That one helper is
  the codebase's single boundary scrub for command text, and every surface
  applies exactly it: `ToolCalled.args`, `CommandPermissionRequested.command`,
  `CommandCheckpointed.command`, and the command-guard trigger-block message
  (`command_permission.rs` `blocked_command_excerpt`). So a new surface showing
  the same command adds no new class of exposure, and a scrubber narrowed to
  one call site would read as a guarantee the other three don't keep. The
  block message additionally orders the excerpt LAST so an OS push preview
  shows the summary and the remedy rather than raw command text. Re-flag only
  as a change to `core::redact_postgres_secrets` itself, applied at every
  surface at once.
  (`crates/lucidos-engine/src/core/mod.rs`,
  `crates/lucidos-engine/src/engine/command_permission.rs`.)

- **The legacy `tailscale serve` form omits `--bg` deliberately, and the
  asymmetry with the current form is the point.** In `serve_arg_forms`
  (`crates/lucidos-app/src/mobile.rs`) the current form is
  `serve --bg --https=443 <target>` and the fallback is `serve https / <target>`
  with no `--bg`. A reviewer (Codex flagged this P1) may read the missing flag as
  an oversight that leaves the fallback running in the foreground under an
  unbounded `output()`. The flag genuinely does not exist on the CLIs the
  fallback targets: Tailscale's 1.52 rework **inverted** the default, and before
  it `serve` wrote persistent config with no foreground concept and no `--bg`, so
  adding the flag would break exactly the pre-1.52 installs the fallback is for.
  The foreground hazard is real but belongs to a different population, a CLI
  between 1.52 and the removal of the old syntax, which parses the positional
  form AND defaults to foreground. That is handled by the deadline in
  `run_serve_attempt` (`SERVE_TIMEOUT`), not by the flag. Re-flag only with
  evidence that a pre-1.52 CLI accepted `--bg`, or that the deadline was removed.
  (`crates/lucidos-app/src/mobile.rs`.)

- **`ProvisionError`'s `From<BoxError>` defaulting every unclassified
  provisioning failure to `Transient` is the deliberate direction, not a silent
  default.** A reviewer reading `crates/lucidos-gateway/src/postgres.rs` sees a
  blanket conversion that assigns a *meaning* (retryable) to an error nobody
  classified, and CLAUDE.md's "No Silent Defaults" rule looks like it applies.
  The asymmetry is the point, and it is ADR-recorded (0014, 2026-08-03): the two
  wrong answers cost wildly different amounts. A wrong `Transient` spends a
  bounded, backed-off retry budget (~2 minutes) and then latches with the same
  message it would have shown immediately. A wrong `Terminal` makes the
  workspace unopenable for the lifetime of the gateway process, which is the
  2026-08-03 bug this typing exists to fix: one `docker run` failure during a
  login race killed both autostart workspaces until the user intervened. The
  default also has to be the safe one for the classification to be extendable at
  all, since an unvisited `?` inherits it. Re-flag only if the retry stops being
  bounded, or with a specific failure that is provably unfixable and still
  arrives through the default rather than through an explicit
  `ProvisionError::terminal`. (`crates/lucidos-gateway/src/postgres.rs`,
  `crates/lucidos-gateway/src/server.rs::provision_failure_action`.)

- **`CredentialStore::get` and `find_by_url` excluding `oauth_client` is the
  contract, not a missed row.** Both carry `AND auth_type <> 'oauth_client'`, and
  a fresh reviewer reads that as a lookup that can silently miss a credential the
  user configured. It is deliberate and load-bearing in two directions. Since
  `20260805134838_drop_credential_name_prefixes_use_auth_type.sql`, `oauth_client`
  is the ONE type permitted to shadow a `service_name` (partial unique index), so
  an unfiltered bare-name `get` would return either row arbitrarily, and the five
  bare-name readers all want the other one: the four provider keys in
  `llm/provider_build.rs` (`anthropic`, `openai`, `openrouter`, `local`) and the
  `apis.json` resolvers. And an OAuth client registration's `auth_value` is a
  `{client_id, client_secret, ...}` JSON blob that is never a usable auth header,
  so `find_by_url` handing one to an outbound request could only leak the secret
  and fail the call. Callers that genuinely want one say so:
  `get_oauth_client` / `get_typed`, and `fetch_required_credential`'s explicit
  second attempt for an `apis.json` entry that names one. Re-flag only if the
  partial index is dropped (making names globally unique again) or if a bare-name
  caller appears that legitimately wants the registration. (`core/credentials.rs`,
  `api/proxy.rs::fetch_required_credential`.)

- **The prefix migration renaming a duplicate to `<name> (unreachable
  duplicate)` is not data loss, and not arbitrary.** Reviewers ask why it does
  not simply leave a colliding pair alone, the way the migration it supersedes
  did. Because leaving it alone INVERTS which row is live: before the migration
  every OAuth read resolves `oauth:<provider>`, so the prefixed row is the live
  registration and the bare one is unreachable by every code path; stranding the
  pair would hand the bare name to the dead row and break refresh on the working
  connection. Stranding is only acceptable when it preserves the status quo. The
  dead row is renamed rather than deleted because a migration must not destroy a
  secret the user typed, and the rename always happens (an occupied archival name
  falls back to a primary-key-suffixed one) because skipping it reintroduces the
  inversion. Re-flag only with a case where the bare row is provably the live
  one. (`migrations/20260805134838_drop_credential_name_prefixes_use_auth_type.sql`.)

- **`normalized_head` stripping an unmatched quote is a deliberate over-block,
  not a false-positive bug.** It trims quote characters off both ends of the
  head token independently, so the head of `'rm -rf /'` (whitespace-split to
  `'rm`) normalizes to `rm` and the command hard-blocks, even though a shell
  would look for a single executable literally named `rm -rf /` and report
  "command not found". A reviewer reasonably proposes stripping quotes only when
  both ends match. That is declined: the over-block costs nothing, because the
  only commands it catches are ones that cannot run either way, while tightening
  the rule is the direction that can LOSE a catastrophic detection, and this
  function is used exclusively by the two danger scans (`segment_is_safe`
  deliberately does not call it). Re-flag only with a legitimate command that a
  shell really executes and this refuses. (`engine/command_guard.rs`.)

- **A raw-text region is bounded but still scrubbed, and that is on purpose.**
  `sanitizeHtmlFragments` skips to the matching end tag for `textarea` / `title`
  / `xmp` / `noscript` / `plaintext`, then recurses into the content rather than
  copying it. A reviewer sees the browser treating that content as inert text and
  proposes passing it through untouched, which would also stop literal markup a
  reader typed into a textarea being altered. Declined: `title` is RCDATA in HTML
  but ordinary markup inside `<svg>` / `<math>`, and the walk does not track
  foreign content, so verbatim is right for one context and wrong for the other.
  Twice on 2026-08-06 a region that skipped the scrub turned out to be reachable
  markup. Re-flag only if the walk starts tracking foreign content.
  (`crates/lucidos-app/src/utils/renderMarkdown.ts`.)

- **`AbortCause::is_transient()` and the `paused` status verdict answer
  DIFFERENT questions and are supposed to disagree.** `RecoveryAfterRestart` is
  transient yet settles the thread at `failed`, which reads as a contradiction:
  two predicates over the same enum, three lines apart, splitting it different
  ways. Declined, and the split is the fix rather than the bug (2026-08-06).
  `is_transient` asks whether a fresh `SessionStarted` is expected, so the
  parent's `active_children_count` must not decrement; it has no actor axis and
  therefore cannot tell the user's own *Switch to new version* from a crash
  sweep. The verdict asks whether anyone is coming back for this turn, which
  ONLY the actor answers, so it keys on `promises_auto_resume` (cause
  `EngineShutdown` **and** a device actor). Keying the verdict on transience is
  what made a crash, and the boot floor handing the Continue button *back*, both
  wear the reassuring pause glyph. The apparent loose end, a transient abort that
  skips the parent decrement while the child reads `failed`, is closed elsewhere:
  `active_thread_statuses()` counts neither `paused` nor `failed`, so the boot
  `rebuild_active_children_count` reconciles the parent either way. Re-flag only
  if `is_transient` gains an actor axis, or if a caller starts deriving a
  user-visible status from it again.
  (`crates/lucidos-engine/src/engine/thread_events/cause.rs`.)

- **`sole_branch_containing` requires no rename evidence, and that is not the
  same laxity as branch adoption's.** The Diff view's worktree-is-gone fallback
  (`api/repositories.rs::resolve_recorded_branch`) locates a thread's work by
  asking which branch contains its last known commit, and a reviewer reasonably
  objects that a sibling cut from the tracked branch before the tracked ref was
  deleted also contains it, so the diff could show commits the thread never
  made. True, and deliberate. The comparison to make is not with *branch
  adoption* (`try_adopt_branch_at_idle`, which demands a reflog rename record
  because it retargets a whole session, including where a later Discard would
  point its `branch -D`) but with the path this fallback stands in for:
  `diff_via_worktree` diffs `base...HEAD` of whatever the worktree sits on and
  has exactly the same property. Demanding more in the fallback would make the
  two disagree about the same repository, and would answer the ordinary
  `git checkout -b`-then-delete case with "no diff" rather than the diff sitting
  right there. It is read-only, the response names the branch it resolved, and
  several candidates already refuse rather than guess. Re-flag only if this
  starts feeding a write (an apply, a branch delete, a change row).
  (`crates/lucidos-engine/src/engine/git_ops/commits.rs`,
  `crates/lucidos-engine/src/api/repositories.rs`.)

- **`in_flight_request_event_id` locks the `active_threads` `std::sync::Mutex`
  in a function that then `.await`s.** Reviews read that as a guard held across
  a suspension point, which on a `std` mutex would block the executor thread and
  can deadlock. It is not held: the guard is a temporary in the initializer of a
  `let`, so it drops at that statement's semicolon, and the value it yields
  (`Option<Uuid>`) is `Copy` and borrows nothing. The `.await` is in the `None`
  arm of the `match` that follows. The borrow checker enforces this rather than
  convention: binding the `&ThreadHandle` instead would fail to compile at the
  `.await`. The same shape is why the nested `handle.request_event_id.lock()`
  inside the `and_then` is safe, and the lock order (`active_threads` outer, the
  per-handle anchor inner) is the only order any caller uses, since the anchor is
  reachable only through the map. Re-flag only if the recorded value stops being
  `Copy`, or if a guard is bound to a named variable that outlives the statement.
  (`crates/lucidos-engine/src/engine/mod.rs` `in_flight_request_event_id`,
  `record_request_event_id`.)

## Frontend

- **`sendCompose`'s catch cannot be reached by a failed chat POST, so a reviewer
  reasoning about "the send failed after the draft was cleared" is describing an
  unreachable state.** The shape is a magnet: `sendCompose` clears the draft, a
  post-send compose write persists that clear, and the catch restores the text
  with `patchDraft` and schedules nothing, so it reads as "the engine keeps an
  empty draft while the composer shows text, and a reload loses it". Two
  independent Codex review passes raised it on 2026-08-06 (once as P1, once as
  P2 via the queued-write variant). What refutes it is `sendMessage`
  (`store/actions/chat.ts`): **it never rejects.** Its only awaits are
  `getWebviewContent()` inside its own `try/catch`, and `sendSlot.waitForTurn` /
  `submitChat(body)` inside a `try/catch` whose every branch (transport error,
  HTTP error, unknown) handles the failure and returns, rendering a failed
  in-thread exchange or toasting. So the only thing that can reject inside
  `sendCompose`'s try is `awaitThreadStarted`, i.e. `POST /threads` failed. On
  that path `startComposeIfNeeded` has ALREADY run `rollbackOptimistic`
  synchronously before the rejection propagates, which deletes the thread from
  `threadMap` and clears the draft, so there is no server row to hold a stale
  clear (`pushNow` awaits the same rejected promise and returns early) and
  nothing to lose. A re-push scheduled there would be dead code. Re-flagging
  this needs NEW evidence that `sendMessage` gained a throw path, not a fresh
  re-derivation from the catch block's shape.

- **Space on a focused choice-card button activates it; it does NOT type a
  space into the prompt.** A reviewer tracing `shouldTypeToFocusPrompt`
  (`hooks/useKeyboardShortcuts.ts`) sees that a bare Space with no modifiers on a
  non-text-input target returns true, notes that a `<button>` is not a text
  input, and concludes that Space on a seeded question option or permission
  button gets swallowed into `execCommand('insertText', ' ')` instead of
  activating the button, contradicting the choice-card contract. The clause that
  refutes it is the one already in that predicate:
  `!(e.key === ' ' && isThreadTranscript(e.target))`. `isThreadTranscript` is
  `el.closest('.thread-content') !== null`, NOT an identity check on the scroll
  region, and every card renders inside `.thread-content` (`ThreadView.tsx`
  renders `renderExchanges(...)` as its children), so a choice button matches and
  the carve-out fires. Enter is unaffected either way. Re-flag only if a card
  surface moves outside `.thread-content` (a portal, an overlay), which would
  break the carve-out for real.
  (`hooks/useKeyboardShortcuts.ts`, `utils/dom.ts` `isThreadTranscript`,
  `components/chat/ThreadView.tsx`.)

- **`threadEntryFocusTarget` omitting the `activeElementIsIdle` guard that its
  sibling seed applies is structural, not an oversight.** A reviewer may note
  that `shouldSeedChoiceFocus` describes four clauses as "a refusal to steal
  focus from something the user is doing", that `threadEntryFocusTarget` applies
  only two of them (hover-pointer, prompt-has-text), and that the one it skips
  guards the path whose target is a permission GRANT button, so an unrequested
  navigation (a sibling thread's `NavigationRequested`, a notification deep link)
  could park focus on "Allow once". The guard is inapplicable there by
  construction: that function runs on a thread SWITCH, where the active element
  is by definition whatever caused the switch (the drawer row just clicked, the
  search result, the deep link), so an idle check is false on EVERY invocation
  and the card would never be reached, including in the ordinary user-clicks-a-
  waiting-thread case the feature exists for. The switch also already moved focus
  before this change: the prior code called `focusIfNeeded(promptEl)`
  unconditionally, so the delta is only WHERE focus lands, never whether it
  moves. The residual risk is bounded by the always-visible ring
  (`@media (hover: hover)` on `.permission-body[data-role="card-choices"]
  .action-btn:focus`) plus the `isElementOnScreen` check, which together mean a
  focused grant button is on screen and ringed. Re-flag only with a real signal
  for "the user initiated this switch" to gate on, not with the asymmetry alone.
  (`components/chat/choiceCardNav.ts`; `docs/plans/2026-08-04-choice-card-keyboard-focus.md`.)

- **`extractLocalFileTarget` claiming a protocol-relative `//host/path` href is
  the accepted residual, and every obvious close is worse.** A reviewer sees an
  extractor documented as "absolute POSIX path" whose `href.startsWith('/')`
  test also matches `//example.com/x`, which HTML resolves as a same-scheme URL,
  and concludes a real external link is being handed to the OS opener. The read
  is right; the consequence is not. On the web path `openLocalFile` is
  `window.open(target, '_blank', 'noopener')`, so a protocol-relative URL opens
  in a new tab, i.e. exactly the correct behavior. And returning `null` for `//`
  makes it strictly worse: the chat handler's terminal guard (ADR 0038) is the
  next branch, so a legitimate external link would be swallowed with a
  "points nowhere" toast instead of opening. Closing it properly means teaching
  the terminal guard about protocol-relative URLs as well, for an href shape no
  model writes (they emit full `https://` URLs, and the bare-URL linkifier only
  matches `https?://`). Re-flag only with a case where the Tauri branch's
  `openExternal` is reached with one, or evidence the shape occurs in practice.
  (`crates/lucidos-app/src/utils/linkifyPaths.ts`,
  `crates/lucidos-app/src/components/chat/ChatExchange.tsx`.)

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
  gap that keeps the focus highlight from being clipped under the fixed header /
  sticky title (commit `9884ece31`) — retargeting the scroll to the pulse panel
  would regress that. The pulse-vs-scroll split is inherent to scoping the
  highlight (fixing the original "whole turn highlighted" bug); the below-fold
  case needs the tall-initiator edge AND is degraded feedback, not a broken
  landing (the user still scrolls to the exchange; the sticky highlight persists on
  the panel). Re-flag only with a concrete fix that keeps the `.chat-exchange`
  scroll-margin behavior intact. (`components/chat/scrollState.ts`.)
  **Amended 2026-08-05:** "only the `.chat-exchange` carries the
  `scroll-margin-top`" is no longer the whole rule. A STEP-level event carries
  its own `data-event-id` on the card that renders it (today the `ResponseFailed`
  failure card), and `chat/response.css` now gives `.chat-exchange [data-event-id]`
  the same clearance at all three breakpoints, so that deep-link scrolls to AND
  pulses the card, with no pulse-vs-scroll split at all. The prior still holds
  unchanged for the two split cases: an exchange-START event (scroll the turn,
  pulse its `.initiator-panel`) and every `data-change-id` landing. A reviewer
  proposing to retarget THOSE scrolls at the pulsed panel is still wrong.
- **The deep-link deadline's recovery scroll does NOT reintroduce the yank that
  `ad48eadad` removed.** A reviewer reading that commit ("Deadline now cleans up
  only instead of forcing a scroll-to-bottom; the prior unconditional fallback
  would yank a user who scrolled to read history during the 4s window") may flag
  the recovery added on 2026-08-05 as undoing it. It doesn't: what that commit
  removed was an UNCONDITIONAL snap, and the recovery is gated on exactly the
  case it named. The scroll runs only when no *user action* was seen since the
  wait began (`watchUserAction`, `utils/userAction.ts`), so a user who scrolled
  away is left where they are; only the warning toast fires for them, being the
  sole remaining signal that the link was dead. The silence the commit left
  behind was itself the reported bug (a notification tap that resolved nothing
  looked broken). Re-flag only if the gate stops covering a real scroll gesture,
  or if the scroll fires on a resolved / superseded claim.
  (`components/chat/scrollState.ts`.)
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
  user has NOT scrolled since the last nav (any scroll gesture retires its ref, even
  while the highlight itself holds), so index
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
- **`refreshThreadEvents` owns its own user-facing surface, and cannot
  reject.** A genuine verdict reaches the user through one keyed card for the
  whole fan-out; a transient rejection (a cancel, the client deadline, a
  stale-connection `TypeError`) is deliberately silent, because the fan-out
  issues one request per loaded thread and the debounced connection dot owns a
  sustained outage. The justifying comments at the call sites are the carve-out
  contract (`.claude/rules/frontend.md` § best-effort telemetry). Because every
  path is caught, a `.catch` on a call to it is dead code rather than a
  silencer: the ones in `connection.ts` and `chat.ts` were removed on
  2026-08-04, and `schedulePendingCleanup` reads its `Promise<boolean>` return
  instead (a `.catch` there had made the force-drop unconditional). Don't
  reintroduce one.
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
- **SSE-driven status signals are `signal<T | null>`, not `Loadable<T>`.**
  `memoryRebuildProgress`, `backupProgress`, `recoveryProgress` and
  `embeddingModelStatus` all follow this. The `Loadable<T>` rule governs a
  view's async DATA source, where "loading" and "failed" must render
  differently from "empty". These are live status feeds whose primary writer is
  the event stream, with a REST snapshot as a secondary writer for a client
  that connected mid-operation; `null` means "nothing known yet", which
  correctly renders as no indicator. There is no fourth state any consumer
  would branch on, and a failed snapshot read is the frontend.md best-effort
  telemetry carve-out (an unsolicited startup/resume probe, self-recovering via
  the next frame). Re-flag only if one of these gains a view that must
  distinguish "still loading" from "read failed".
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

- **The file preview modal closes on a `panelOverlay` IDENTITY change, and that
  is not over-eager.** `FilePreviewModal`'s `useSignalEffect` compares
  `panelOverlay.value` against the object captured when the preview opened, so a
  reviewer reasonably asks what happens when the overlay is re-set to an
  equivalent value: the modal would close for a non-navigation. Every writer of
  `panelOverlay` was walked, and there is no such writer. Each one is either a
  real navigation (`openApp`, `openFilePreview`, `openUrl`, the form/notification
  openers, `setActiveMenu`), a teardown (`entityReferences` nulling the overlay
  when the open app/file is deleted underneath), or history restore
  (`navigation.restoreState`), and in all three, closing a glance layered over
  the old pane is the wanted behaviour, Back included. The app-refresh path that
  WOULD re-set an equal app-ui overlay does not exist: `AppUiRefreshRequested`
  goes through `appRefreshKey`, never `panelOverlay`. A value-equality check
  would be strictly worse here, since it would keep the glance alive across a
  genuine re-navigation to the same file. Re-flag only if a writer appears that
  re-sets `panelOverlay` for a non-navigation reason.
  (`components/files/FilePreviewModal.tsx`, `store/actions/filePreviewModal.ts`.)

- **The preview modal mutating `filePreviewSource` / `selectedLines` /
  `lineScrollTarget` / `filePreviewEditing` is a scoped BORROW, not shared-state
  leakage.** Those four are the Files panel's view state, and the modal writes
  all four on open, which looks like one surface reaching into another's state.
  It snapshots them first and restores them in `closeFilePreviewModal`, and the
  panel's preview is never mounted while the modal is up (the modal is reachable
  only from an app iframe, so the content pane is showing that app), so there is
  no concurrent reader. The one close that does NOT restore is the
  navigation-triggered one (`{ navigated: true }`), and that asymmetry is
  load-bearing rather than an oversight: `openFilePreview` clears the selection,
  the scroll target and the source toggle BEFORE it sets `panelOverlay`, so by
  the time the modal's watcher sees the overlay change the borrowed signals
  already belong to the destination, and handing the snapshot back would paint
  the panel's pre-modal highlight onto the file just opened. The escalation
  closes before it navigates, so it restores normally. It is deliberate over threading a selection + scroll +
  source-mode triple as props through `FilePreviewInline`, `RepoFileText` and
  `LineNumberedCode`, which would widen the panel's own code for no panel
  benefit. Pinned by the restore cases in
  `store/actions/filePreviewModal.test.ts`. Re-flag only if a second surface can
  read those signals while the modal is open.
  (`store/actions/filePreviewModal.ts`, `docs/plans/2026-08-05-file-preview-modal-from-an-app.md`.)

- **The navigation focus marker's entrance bloom does NOT revert `96b2c8e2a`,
  which removed an entrance animation "per user request".** A reviewer running
  `git log` on `styles/global/host-components.css` will find that commit deleting
  a `nav-focus-fill` keyframe at the user's explicit ask, then find
  `nav-focus-spotlight-on` added back on 2026-08-05, and reasonably call it a
  silent reversal. It is not: the user asked for the bloom directly in the thread
  that repainted the marker from an outline frame into a background highlight, as
  an approved fork of `docs/plans/2026-08-05-nav-focus-marker-spotlight-highlight.md`
  ("Approve, with the bloom"). A later explicit instruction outranks an earlier
  one. The two animations are also not the same thing, which is why the old
  request does not carry over, and the distinction is the DESTINATION, not the
  direction: `nav-focus-fill` ramped DOWN to **transparent**, so the fill was
  purely an entrance flourish and missing it (a glance away, a slow iOS load)
  meant missing it entirely, with only the border left behind.
  `nav-focus-spotlight-on` ends at the marker's **persistent resting wash**, which
  then stays until the user acts. A look-away can miss the turn-on ramp; it
  cannot miss the marker. Be precise about what IS given up, so a reviewer reading
  `96b2c8e2a`'s literal words ("shown instantly on landing") finds them addressed:
  instant-on is genuinely gone, and that is the deliberate content of the later
  request. (The first cut of the repaint was additionally
  decay-only, starting brighter and settling down, so it was never missable at
  any frame. The user then asked for a real turn-on, and it ramps up from
  transparent as of 2026-08-05. The persistence is what makes that safe, and it is
  the property to defend: a ramp ending in nothing is the thing `fd61d7af9`
  removed.) The ramp's shape is pinned by
  `components/shared/__tests__/nav-focus-marker-paint.test.ts`. Known and accepted
  alongside it: `⌘↑`/`⌘↓` turn-nav marks a different element per press, so
  stepping through a transcript turns on once per press, which is wanted here (it
  is what draws the eye to the new landing). Re-flag only if the user asks for the
  entrance animation to go away again. (`styles/global/host-components.css`.)

- **A test that awaits a frame under `vi.useFakeTimers()` is NOT broken by
  `src/test-setup.ts`'s synchronous `requestAnimationFrame` stub.** A reviewer who
  finds that stub (`(cb) => { cb(0); return 0; }`) will conclude that any
  same-tick assertion made after an event which schedules an rAF must fail,
  because the callback would already have run. It does not, for two independent
  reasons: the stub is installed only `if (typeof globalThis.requestAnimationFrame
  === 'undefined')`, and `vi.useFakeTimers()` replaces `requestAnimationFrame` with
  a timer-driven one for the duration of the suite regardless, so
  `vi.advanceTimersByTime(n)` is what runs the callback. Verified by probe on
  2026-08-05 (a throwaway spec asserting the callback has NOT run immediately after
  scheduling, then HAS run after an advance, passed on both counts). The canonical
  users are the navigation focus marker's ref-lifetime cases in
  `components/shared/__tests__/focusMarker.test.ts`, which assert that
  `navFocusElement()` survives the dismissing event and is null one frame later.
  Re-flag only with evidence that the Vitest config stopped faking rAF, in which
  case those tests fail loudly rather than silently.
  (`crates/lucidos-app/src/test-setup.ts`, `components/shared/__tests__/focusMarker.test.ts`.)

- **The navigation focus marker's hold is wall-clock, and it running out in a
  hidden tab is an accepted limit, not a bug.** A reviewer will notice that
  `NAV_FOCUS_HOLD_MS` and then `NAV_FOCUS_FADE_MS` both keep counting while the
  document is hidden (`setTimeout` is throttled in a background tab but still
  fires, and the class removal is the JS timer's job, not the animation's), so
  landing, pressing a key, and immediately switching tabs means returning ~3s
  later to no marker. That is real, and it is deliberately not fixed. It is not
  a regression: before the hold existed the same keypress dismissed the marker
  outright and it was gone 0.4s later, so the hold strictly lengthened the
  window. It is not the case the persistence guarantee covers either
  (`fd61d7af9` protects a user who has NOT engaged; this path opens with a
  keydown, which has dismissed the marker by design since `96b2c8e2a`). And the
  fix, tracking `document.visibilityState` so the hold measures visible painted
  time, adds another pausable clock to a module in which every bug found so far
  has been a clock-interaction bug (the stale ref across the dissolve, the frame
  stranded by hidden-tab timer ordering, the hold anchored before the ramp
  instead of after). Re-flag only with a report of it actually bothering someone,
  which would justify the added state. (`components/shared/focusMarker.ts`.)

- **Cutting `NAV_FOCUS_FADE_MS` to 0.8s does NOT undo the round that lengthened
  it.** A reviewer running `git log` on `components/shared/focusMarker.ts` will
  find the dissolve walked 0.4s to 1.5s to 2.5s across three rounds of direct
  user feedback, the last of them answering "the light turns down almost right
  away", and then find it cut to 0.8s, and reasonably call it a silent reversal
  of a user request. It is not, for two reasons. A later explicit instruction
  outranks an earlier one, and this one was explicit ("turning off the lights
  should be a little faster ... make it maybe 800ms"). More importantly the two
  requests are about different phases: what actually answered "turns down almost
  right away" was `NAV_FOCUS_HOLD_MS`, the guaranteed 2s at FULL brightness added
  in the same round, and the hold is **untouched** here. Lengthening the dissolve
  alongside it was the part that overshot, because the dismiss is triggered BY the
  action that moves the user on, so a long drain is time the marker spends going
  out after they have stopped looking. Time at full is the hold's job; the
  dissolve only has to avoid blinking off. The relationship is pinned both ways by
  `nav-focus-marker-paint.test.ts` (slower than the turn-on, and at most a
  second). Re-flag only if the user asks for a slower dismiss again, or if a
  change cuts the HOLD, which is the value that request was really about.
  (`components/shared/focusMarker.ts`, `styles/global/host-components.css`.)

- **The login agent's install swallows every failure on purpose, and that is
  not the fail-fast violation it looks like.** `ensure_login_agent_installed`
  (`crates/lucidos-app/src/desktop.rs`) logs and returns on a plist it cannot
  build, resolve, write, or bootstrap, which reads exactly like the
  "log-and-proceed instead of failing fast" pattern `/harden` Phase 2.5 hunts
  for. The difference is what the dependency is *for*: the login agent buys the
  NEXT boot a client in the menu bar, and nothing in the running session depends
  on it. Failing the client's startup (or its service install, which happens on
  the same path) over it would cost the user a working app to gain nothing, and
  the degraded state is not silent-and-broken but simply the behaviour every
  build before 2026-08-06 had, namely opening the app by hand. The one failure a
  user would actually chase, `open` never succeeding at login, does say so:
  the script echoes why it gave up to `client-login.err.log`. Re-flag only if
  something in the live session starts depending on that agent.
  (`crates/lucidos-app/src/desktop.rs` `ensure_login_agent_installed`,
  `login_launch_script`.)

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

- **`sloc.awk` resets string state at every line break on purpose, and the
  multi-line-string misreads that follow are the smaller error.** A reviewer
  notices `classify()` takes one line at a time, so a line inside a Rust raw
  string or a JS template literal that begins with `//` is booked as a comment,
  and reasons that an unterminated `/*` in such a string could eat the rest of
  the file. Both readings are correct about the mechanism and wrong about which
  way to trade. Measured 2026-08-03: 57 Rust lines and 12 TypeScript lines
  tree-wide, 0.02% of code, and 49 of the 57 are `//` comments inside JavaScript
  embedded in a raw string, so they are comments by any reading. Carrying the
  state means matching `r#"` to the `"#` with the same hash count and tracking
  unterminated `"` across lines; getting that wrong books an entire file as
  code and silently deletes its comment count, which is orders of magnitude
  worse than 69 lines. Per-line reset confines every misparse to one line. The
  runaway direction is already covered: the unterminated-block canary exits
  non-zero and names the file, and it is silent across this whole tree. Re-flag
  only with a measurement showing the multi-line share has grown materially, or
  with a tracker whose failure mode is bounded. (`.claude/skills/project-stats/sloc.awk`
  header § KNOWN LIMITS, fixtures in `sloc_test.sh`.)

- **`sloc.awk` closes a Rust block comment at the FIRST `*/` even though Rust
  block comments nest, and that is measured, not overlooked.** The language rule
  is real, so a reviewer correctly observes that `/* outer /* inner */ tail */`
  leaves ` tail */` read as code. Depth-counting was implemented on 2026-08-03
  and reverted the same day on the numbers. This tree contains no nested block
  comment; the only construct that ever incremented the counter was a false
  `/*` inside the glob `'**/*.md'` in the system-prompt raw string
  (`engine/chat/process/system_prompt.rs`, opened at the `r#"` on line 433).
  Because closing then needed two `*/`, that single-line misparse ran on past
  its own line and moved 5 lines of prompt text from code to comment, all of
  them wrong. A false `/*` inside a string is common (globs, regexes, URLs) and
  a nested comment is rare, so depth-counting amplifies the frequent error to
  fix the rare one, and it forfeits the per-line confinement the entry above
  depends on. Re-flag only alongside cross-line string tracking, which would
  remove the false opens first, or with a real nested comment in the tree.
  (`.claude/skills/project-stats/sloc.awk`, the `rust: block comments close at
  the first */, deliberately` fixture in `sloc_test.sh`.)

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
  **That bounded poll now EXISTS, for a second and distinct race (2026-07-31).**
  The publisher does publish before the rc push (`release.sh` blocks on
  `scripts/lib/release_rc_front_door.sh` until the origin serves the candidate),
  but that wait polls from one machine and therefore observes exactly ONE
  Cloudflare POP, while a runner resolves to another whose edge cache can still
  hold the previous release's copy. v0.18.3 and v0.18.5 both reddened a
  front-door leg on that alone, and on v0.18.5 only `macos-latest` reddened
  while `macos-15-intel` and `ubuntu` read the correct version off the same
  origin, which is a per-POP cache rather than an origin regression. Rung 1
  therefore re-reads a mismatch, and only a mismatch on the push arm, to a
  bounded budget with a cache-busting query nonce plus no-cache headers, warns
  loudly when it converges late, and still exits on expiry. So a reviewer
  proposing "retry the version check" is describing what is already there: the
  live question is only whether the budget or the cache-busting still fits the
  observed lag, and the arming wait's single vantage point stays deliberately
  unchanged so the next failure of this shape is unambiguous.
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

- **The app iframe's `body` carries an explicit `font-size` and the host's does
  NOT. That asymmetry is deliberate, and deleting it to "restore parity" is a
  bug that has already shipped once.** The two files mirror each other by
  construction (`sdk_iframe.css` repeats base.css's token blocks under "keep in
  sync with base.css" markers), so a reviewer comparing them side by side reads
  the extra declaration as drift. It is not, because the two documents are not
  symmetric in what fills them: every text element in the host shell is
  explicitly sized from a `--font-size-*` token, so the host body's computed size
  is a value nothing actually renders at, whereas an app's prose is whatever the
  app author wrote, and anything they did not size falls straight through to it.
  Leave the declaration off and that fallthrough is the raw root (`1rem`, 18px at
  a 112.5% UI scale) against a chat message body of `--font-size-sm` and a
  documented app body step of `--font-size-md`. That is exactly what happened:
  `2a742266b` (2026-06-19) dropped a `body { font-size: 0.875rem }` reasoning
  that it "made app text permanently smaller and the user's UI-scale preference
  look ignored", and both halves were wrong. `0.875rem` is a `rem`, so it tracked
  the root the whole time, and 14px was at the TOP of the host's range rather
  than below it. The measurement was against the host body's *computed* size
  instead of against any pixel the host paints. Restored as `--font-size-md`, the
  documented body step, on 2026-08-05 after a user reported apps rendering a
  scale step larger than their threads. Pinned by
  `api::sdk::tests::iframe_body_is_sized_from_the_type_scale`. Re-flag only if
  the host shell starts rendering real body text at the unstyled root size.
  (`crates/lucidos-engine/src/api/sdk_iframe.css`, `styles/global/base.css`.)

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

- **`emit_or_log` after a committed write is the engine's contract, not a
  dropped error.** Reviewers see a store method commit its row in one
  transaction and then emit through `emit_or_log`, and flag that a transient
  failure inside `emit` is swallowed while the method still reports success,
  so the write can land without its projection/SSE event. Accepted: every
  `SystemEvent` emitter in the engine works this way (`emit_or_log` logs under
  `[EventBus] <ctx> emit failed`), `EventBus::emit` owns its own transaction so
  there is no caller-owned tx to join, and propagating would report failure for
  a write that actually succeeded, sending the user into a retry against a row
  that already exists. `RepositoryStore::register` is the worked example: the
  cost is one missed live refresh, and nothing durable is lost because a live
  repo's name resolves from the `repositories` row itself and the next client
  load refetches the list. Re-flag only where the emit is the ONLY record of
  the state change (nothing re-derivable from the committed row), or once
  `EventBus` accepts a caller-supplied transaction.
  (`crates/lucidos-engine/src/core/repositories.rs`,
  `crates/lucidos-engine/src/engine/event_bus/mod.rs`.)

- **A store mutator taking `&EventBus` is the design, not a layering
  violation.** Reviewers see `core/` stores depending on the engine's EventBus
  and flag the inversion, or propose returning the event for the caller to
  emit. Accepted, and load-bearing: an event the caller emits is an event the
  next caller forgets, which is precisely how an agent-registered repository
  stayed invisible in every client's list. ADR 0032 makes the write path own
  the announcement, `core::announced_surfaces` classifies every surface, and
  source-scan tests fail a reachable writer that does not emit. Re-flag only
  with a concrete alternative that keeps the emit unskippable.
  (`crates/lucidos-engine/src/core/announced_surfaces.rs`, `docs/adr/0032-a-state-write-owns-its-announcement.md`.)

- **A new table needs a registry entry, and "it has no consumer yet" is a
  classification, not a reason to skip one.** Reviewers propose deferring the
  decision until something listens. The registry's `Silent { reason }` arm is
  that decision, recorded, and the completeness test refuses a table without
  one. The point is that the next person re-decides instead of re-discovering.
  (`crates/lucidos-engine/src/core/announced_surfaces.rs`.)

- **`POST /api/v1/triggers/run` stamps no actor, and that is the feature.**
  Reviewers read `.claude/rules/rust.md` ("mutating endpoints stamp the
  actor") and flag the handler. Two reasons it does not apply. The handler
  emits no `SystemEvent` of its own: the run's `TriggerExecuted` /
  `TriggerCompleted` come from the queue executor, exactly as for a scheduled
  fire. And an actor on those events is precisely the tell that would make an
  *off-schedule run* distinguishable downstream, which the design says it must
  not be (nothing has to learn a third kind of run, and `catch_up_decision`
  keeps reading `last_run` as "did this work happen"). Re-flag only alongside
  a decision to make manual runs distinguishable, which is a payload addition
  to `TriggerExecuted`, not a change to this handler.
  (`crates/lucidos-engine/src/api/triggers.rs`,
  `crates/lucidos-engine/src/engine/engine_impl/trigger_runs.rs`,
  `docs/plans/2026-08-02-trigger-run-action.md`.)

- **The run action's in-fire recursion guard is in `trigger_runs`, not in
  `check_scheduling_tool_in_trigger`, and it does not cover the HTTP path.**
  Reviewers notice `run_trigger` missing from the scheduling-tool guard's match
  arm, or that a script trigger can reach the run endpoint over HTTP where the
  `ACTIVE_TRIGGER_ID` task-local is unset. Both are known. The guard is
  stricter than that function's contract (it refuses self-id too, because
  self-run recurses where self-pause terminates) and has to hold for the HTTP
  and CLI surfaces, which never reach that function, so stating it twice would
  drift. The HTTP gap is bounded by design: a trigger asking to run itself is
  already active, so the cron fire coalesces and comes back as
  `already-running`. Re-flag only with a way to propagate trigger context
  across the HTTP boundary.
  (`crates/lucidos-engine/src/engine/tools/scheduler.rs`,
  `crates/lucidos-engine/src/engine/engine_impl/trigger_runs.rs`.)

- **`upload_staged_assets` still `--clobber`s its artifacts while the previous
  `latest.json` is live, and that is the deliberate residual.** Reviewers see
  `gh release upload … --clobber` and note that on a corrective re-upload
  (`release.sh --attach-notarized` swapping in the stapled DMG) the release
  already carries a manifest, which stays readable for the seconds its payload
  is being deleted and re-uploaded. That is true, it is documented at the site,
  and the two obvious closes are worse: removing the manifest first turns a
  possible 404 on the payload into a guaranteed one on the manifest for the
  whole upload, and skipping artifacts already on the release needs an identity
  test GitHub does not offer (it exposes no asset checksum, and name-plus-size
  would admit shipping a stale updater payload). The change that mattered is
  done: the FIRST publish no longer advertises a payload before it exists, which
  is where the 8h06m v0.16.0 window came from. Re-flag only with a way to make
  the re-attach upload just the DMG, which is a change to what `release.sh` asks
  for rather than to how this function honours it.
  (`scripts/lib/release_upload.sh`, F8 in
  `docs/audits/2026-08-02-macos-update-path-audit.md`.)

- **A Settings `data-search-anchor` must render UNCONDITIONALLY, never inside a
  `Loadable`'s loaded branch.** This entry was first written the other way round,
  dismissing the concern, and the escape clause it left ("re-flag only with a
  caller that can realistically fire before its target's data loads") was
  satisfied within the same change: adding `access:network` and
  `coding-agents:repositories` to the search index gave both anchors a caller,
  Search Everywhere, that fires from a COLD Settings open. `SettingsView`'s
  scroll effect does one `querySelector` on the commit where the subview mounts
  and then clears `settingsScrollTarget` whether or not it matched, and its deps
  (`[settingsScrollTarget.value, settingsSubview.value]`) do not change when a
  child's fetch lands, so there is no retry: the jump silently drops the user at
  the top of a long page. Both sites now render the section header (and its
  anchor) in every state and gate only the body. A source scan cannot catch a
  regression here (an anchor inside a never-taken branch still reads as
  present), so this ledger entry is the guard: when you add an anchor, check it
  is outside the Loadable.
  (`crates/lucidos-app/src/components/settings/NetworkAccessPage.tsx`,
  `SettingsView.tsx` `repositoriesSection`.)

- **`hasRenderableResponseContent` counts a `step` / `event_wait` as drawn even
  though `renderResponseEvents` gates those on `showSteps`.** Reviews read that
  as an incomplete mirror: with steps collapsed the renderer returns `null` for
  them, so a boundary holding only a step would supposedly still open an empty
  panel. It does not. `getEventToggleState`'s `showStepsToggle` is the same
  `some(e => e.type === 'step' || e.type === 'event_wait')`, so a step present
  means the body always renders the "Show steps" button: visible content, and
  the affordance that reveals the rest. The predicate is about whether a panel
  is worth opening, not about which of its rows are currently expanded, and
  threading `showSteps` into it would make an exchange's panel appear and
  disappear as the user toggles a global preference. The narrower shape the
  concern usually reaches for, a lone `CodingAgentToolResult`, produces no step
  at all: that arm only resolves a pending step, it never pushes one. Re-flag
  only if `showStepsToggle` stops keying on the same predicate, which would make
  the button genuinely absent.
  (`crates/lucidos-app/src/store/thread-events/exchange-render.ts`,
  `store/event-rendering.ts` `getEventToggleState`.)

## Settled architecture questions

- **No shared turn-lifecycle orchestrator across the agent-session loop and
  the chat agentic loop** — ADR 0003. The seam already exists
  (`lifecycle.rs` pure decision functions + the typed terminal helpers).
- **External-repo coding-agent threads stay out of the change/dot/blocking
  machinery** — ADR 0001.
