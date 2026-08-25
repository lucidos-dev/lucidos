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

- **Permission grants living under `<workspace>/.lucidos/` being agent-writable
  is the recorded non-property, not an escalation this introduced.** A reviewer
  reading `core/grants/mod.rs` correctly observes that an agent with `run_bash`
  can append to the very file gating it. Codex flagged exactly this as P1 on the
  branch that moved the files there.

  ADR 0095 decides it in as many words. Every workspace shares a uid and a
  shell, so there is no containment to be had. The property bought is semantic,
  that no decision binds outside the context it was made in. What `.lucidos/` buys over `data/config/` is narrower and real:
  the *engine's own file tools* refuse it, asserted in
  `the_file_tools_cannot_address_a_permission_grant_file`.

  The move also does not widen the escalation. The command guard classifies an
  out-of-workspace **append** as safe and wanted (`command_judge.rs`). So
  `>> ~/.lucidos/agent-allowed-commands` was ungated before this change, and
  appending is all it takes to add a grant. Only out-of-workspace *destruction*
  was gated, and truncating a grant file removes grants. Re-flag only if the
  design gains a containment claim, or if workspaces start running under
  separate uids.

- **An orphaned build outliving its `build-slot` wrapper is a weighed
  trade-off, not an oversight.** `spawn_child`
  (`lucidos-cli/src/build_slot.rs`) runs the build as a child and holds the
  slot's flock in the parent. So a reviewer correctly observes that killing
  the wrapper alone frees the slot while `cargo` compiles on. Codex flagged
  exactly this on the branch that added it.

  Two things make it narrow. The child deliberately stays in the wrapper's
  process group, so every ordinary kill reaches both: a terminal Ctrl-C, and
  the engine's `BuildProcessGroupGuard` on a coalesced Apply. The escape
  needs a deliberate single-pid SIGKILL. The OOM killer will not do it, since
  it picks the largest RSS and the wrapper is tiny.

  The fix a reviewer reaches for is worse. Clear close-on-exec so the child
  inherits the lock, and a leaked grandchild pins that slot until someone
  kills it. That is the stale-holder failure the whole design avoids. Re-flag
  only if the child gains its own process group, or if the wrapper starts
  outliving the build. (ADR 0070 § Consequences.)

- **`cancel_event_wait`'s `on` ending a whole multi-type subscription is the
  intended reading, not a leak.** `LiveWait::watches` (`event_wait/mod.rs`) is
  an `any` over the `on` list, so a wait armed with `--on A --on B` answers yes
  to both and `cancel --on A` takes the whole row. A reviewer reasonably reads
  that as contradicting the verb's promise to leave other watches alone (Codex
  flagged exactly this on 2026-08-10). It does not: a wait is ONE rendezvous
  with several triggers, spent by the first match, so there is no `B` leg left
  to be woken by once `A` is stood down. Keeping "the rest" would mean
  replacing it with a subscription the caller never armed, and nothing in this
  family mutates a wait: the persisted `EventWaitStarted` IS the wait, and
  there is no update verb. The report is honest about it, because
  `describe_subscriptions` names every event type it ended. Re-flag only if a
  wait stops being one-shot, or if the surface grows a way to narrow one.
  (ADR 0059 § Alternatives; `event_wait/agent_surface.rs`.)

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
- **The CC fast-path follow-up send has no TOCTOU, and needs no counter.** In
  `chat/process/run.rs` the session lookup, the `is_live()` check and the
  `msg_tx.send` all run under one `agent_sessions.lock().await`, and the run
  loop's idle decision takes that same lock, so the external watchdog can't
  remove the session mid-sequence and the idle decision can't observe a
  half-sent follow-up. That is also why `msg_rx.len()` is an exact answer to
  "is a follow-up still unread" at the idle decision rather than a sample:
  an unbounded-channel send is synchronous, so under the lock a message that
  was sent is a message that is in the channel. A send that fails put nothing
  anywhere, so there is nothing to roll back. (Until 2026-08-07 the send site
  also bumped a `pending_followups` counter with a rollback arm; the counter
  guessed at what the channel could be asked directly, and the guess broke for
  Claude Code, which merges forwarded inputs into a single Result.)
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
  bare-name readers all want the other one: the provider keys in
  `llm/provider_build.rs` (`anthropic`, `openai`, `openrouter`, `xai`, `local`) and the
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

- **The cron never-fires guard SHOULD reject a schedule whose next match is past
  the crate's year-2100 horizon.** `validate_cron_expressions`
  (`engine/tools/scheduler.rs`) rejects an expression when
  `Schedule::upcoming(tz).next()` is `None`, and a reviewer reasonably objects
  that `cron`'s `Years` unit stops at 2100, so near that boundary the iterator
  exhausts for schedules that are calendar-satisfiable (e.g. `0 0 9 29 2 *`
  queried after Feb 2096, since 2100 is not a leap year) and the guard calls them
  impossible. The objection is factually right about the horizon and wrong about
  the consequence: **the scheduler consults the same oracle**. `run_task_loop`
  (`scheduler/task_runner.rs`) calls `next_occurrence_multi` each pass and returns
  `Ok("no more occurrences")` on `None`, and `TriggerConfig::next_run` reports
  nothing upcoming. So a horizon-blocked trigger genuinely does not fire under
  this engine, and accepting it would recreate precisely the silent non-firing
  the guard exists to eliminate. Rejecting keeps guard and runtime in agreement,
  which is the property that matters; the fallback diagnosis names the horizon
  (`CRON_SEARCH_HORIZON_YEAR`) so the message is not a dead end. Re-flag only if
  the runner stops using `upcoming()` (e.g. gains its own unbounded search), or
  if the crate's year bound is lifted, at which point the two move together
  anyway. (Raised by a Codex review on 2026-08-07.)

- **A `data/`-relative path helper that splits on `/` alone is NOT a Windows
  bug, because there is no Windows build.** Reviews flag any path predicate or
  splitter that assumes a forward slash (`is_vendored_path` in
  `core/artifacts.rs` is the current example) on the theory that
  `Path::to_string_lossy` would yield `\` separators on Windows and the check
  would silently pass everything. Lucidos ships two shapes, the macOS
  `.app`/`.dmg` and the headless tarball for macOS + Linux; `install.sh`
  refuses any OS other than Darwin or Linux and no release carries a `.msi`
  (`CLAUDE.md` § One-Click Install). Every producer of these strings is
  `list_searchable_data_files`, which builds them as
  `format!("{prefix}/{rel}")` on a Unix host. A stray `\`-normalizing call
  elsewhere (`knowhow::id_from_path`) is historical, not evidence of support.
  Re-flag only if a Windows target lands, at which point the fix is one
  normalization at the walk, not per-predicate. (`core/artifacts.rs`,
  `engine/chat/process/workspace_payload.rs`.)

- **Binding an `i64` into `make_interval(secs => $n)` is correct, not a
  reinterpreted float.** `make_interval` declares `secs` as `double precision`,
  so the event-wait window queries look like they hand Postgres eight bytes of
  int8 to read as a float, which would collapse a 180-second window to a
  denormal near zero and silently empty it. Flagged that way by a Codex review
  on 2026-08-07. It is not what happens: sqlx declares the parameter's type in
  `Parse` from the bound Rust type, so Postgres resolves
  `make_interval(secs => int8)` and applies the standard implicit int8 to
  float8 cast to the *value*. There is no bit reinterpretation across a
  declared type boundary. Measured directly through sqlx's binary protocol:
  `SELECT EXTRACT(EPOCH FROM make_interval(secs => $1))` with an `i64` 180
  returns exactly 180. `the_lookback_window_is_measured_by_the_database_clock`
  is also two-sided on this by construction, since it puts one row 120s back
  and one 240s back and demands exactly the first: a zero interval would
  return neither and an oversized one both. Re-flag only with evidence that
  sqlx stopped sending the parameter type, which would show up as that test
  failing rather than as a reading of the SQL.
  (`engine/event_wait/mod.rs`, `engine/event_wait/register.rs`, ADR 0053.)

- **`tool_arg_entity_repair` decoding the same key for one tool and not
  another is the design, not an inconsistency.** A reviewer sees `name`
  rewritten for `trigger_groups` and untouched for `proxy_request`, or
  `summary` rewritten inside an `emit_event` payload while its sibling `name`
  is not, and reads the asymmetry as an oversight. The allow-list is scoped per
  TOOL precisely because the same word is prose in one schema and an identifier
  in another: a trigger group's `name` is its label, `proxy_request`'s resolves
  an entry in `data/config/apis.json`, and `env_vars`' is a variable name. A
  tool with no row is declined whole, which is also how every third-party
  `mcp__<server>__<tool>` is handled: by never having claimed to know its
  schema, rather than by a special case. Entries are **full paths from the
  argument root**, not leaf keys, so there is no exclusion list to check
  against and no "but a nested X also matches" gap: `new_value.message`,
  `env.name`, `on.condition.name` and `payload.details.summary` are simply not
  the listed paths. An array index is not a path segment, so
  `questions.options.label` covers every option. Re-flag only with a listed
  (tool, path) pair whose value is demonstrably an identifier or opaque data
  rather than prose the user reads.
  (`engine/tool_arg_entity_repair.rs`, `docs/temporary-measures.md` §2.)

- **The merge-ownership guard letting an apply through during the Tier-2 /
  Tier-3 startup window cannot move `main`** (2026-08-11, Codex). Those tiers
  open the `MergeConflictDetected` pairing inside their spawned task, before
  `run_direct_agent` registers the session, so for a moment
  `decide_merge_ownership` sees an open pairing with no resolver named and
  returns `CallerMayMerge`. That reads as the guard's whole premise leaking. It
  does not merge anything: arriving at a resolution at all means
  `catchup_and_ff_to_main` ALREADY failed on this branch, and in that window
  nothing has merged yet, so a concurrent apply's ff fails identically and it
  spawns another merge attempt rather than advancing `main`. The alternative,
  an in-memory claim held across the spawn, is the wedging shape ADR 0060
  rejects by name. Tier 1 has no window at all: it binds the session before it
  opens the pairing. Re-flag only with a path where the ff can SUCCEED between
  the pairing emit and session registration.
  (`engine/change_ops/mod.rs`, `engine/change_ops/apply.rs`, ADR 0060.)

- **`BACKGROUND_PROCESS_RULE`'s `/tmp/$(basename "$PWD").log` example is unique
  per session, not a shared path** (2026-08-13, Codex). The rule tells the agent
  to redirect a long command's output so each blocking `TaskOutput` stays cheap
  (it replays the task's whole accumulated output on every call), and builds the
  example path from the worktree's basename. A `/tmp` name reads as
  collision-prone across concurrent sessions. It is not: the engine prompt
  reaches only engine-spawned sessions, and every one runs in
  `deterministic_worktree_path`'s `thread-<short_thread_id>` directory, whose
  collision namespace is `(workspace, thread)`. It deliberately does NOT use the
  worktree-local `.lucidos/` that `/harden` and the `run-e2e` skill write to:
  those are Lucidos-checkout-only, while four of the seven prompt flavors run in
  an external repo or an app worktree, where an untracked `.lucidos/` log would
  dirty someone else's tree. Re-flag only if worktrees stop being named per
  thread. (`agent_session/prompts.rs`, `agent_session/resume.rs`.)

- **A `continue` in the agentic loop's no-tool-calls branch cannot burn the
  tool-call cap** (2026-08-13, Codex). Flagged against the *wake check*. The
  worry is a turn that has just exhausted `max_tool_calls` losing its final
  answer. The forced round would hit the cap check and return the generic cap
  message, with no model call behind it.

  It cannot happen, and the reason is where the counter moves.
  `tool_calls_made += 1` fires at exactly one site, inside the tool-execution
  path of the NON-empty branch. So the empty branch is entered holding whatever
  count the top of that round held. That count already passed
  `tool_calls_made >= max_tool_calls`, or the loop would have returned before
  the model was called at all.

  The forced round therefore sees the same count and is not capped. The round
  backstop is not a route either: it is `max_tool_calls + NON_TOOL_ROUND_SLACK`,
  and the slack is 100, sized for exactly this class of non-tool `continue`.
  Re-flag only if the empty branch starts advancing `tool_calls_made`, or if the
  slack is removed. (`engine/agentic_loop/run.rs`, `NON_TOOL_ROUND_SLACK` in
  `helpers.rs`.)

- **The branch-create retry reusing one worktree path cannot strand itself on a
  leftover directory** (2026-08-13). `FreshBranch::create_worktree`
  (`agent_session/spawn.rs`) loops on the same `wt_path`, re-deriving only the
  branch name. A reviewer reasonably asks what happens if the losing
  `git worktree add -b` left the directory behind: the next attempt would fail
  on "path already exists", which `branch_name_is_taken` deliberately does not
  match, so the spawn would die with a confusing error.

  Git does not leave it. A name-taken failure aborts before the directory
  exists, verified directly against git 2.50.1. It is also exercised by
  `a_lost_branch_race_retries_until_it_wins`, where one task retried six times
  at one path and succeeded. Do not "fix" this by clearing the directory inside
  the loop: that puts a destructive call on a retry path, against the
  failure-path cleanup rule, to no purpose. Re-flag only with a git version
  that leaves the path behind on a name-taken abort.
  (`agent_session/spawn.rs`, `git_ops/branch_name.rs`.)

- **A Stop that cancel-stamps a question and hits `SESSION_ALREADY_WAITING`
  cannot strand the row at `running`** (2026-08-16). `claude_code_stop`
  (`api/claude_code.rs`) resolves the pending card first, and
  `UserQuestionAnswered` moves the projection to `running` whatever the answer
  kind. `interrupt_agent` then returns that `Err` for an idle session, without
  settling. A reviewer reasonably reads a permanent `running` row out of the
  pair.

  The state it needs is unreachable. A pending card means the asking call is
  still blocked, in the PreToolUse hook for Claude Code or the MCP call for
  Codex. That block is what stops the turn completing, so the session cannot be
  `is_waiting`. Every other route resolves the card first, a follow-up
  superseding it and the preserve contract withholding Continue from a
  question-parked thread. Re-flag only if a backend gains a way to finish a turn
  with its question call still open.
  (`api/claude_code.rs`, `engine/claude_code/control.rs`,
  `engine/agent_question.rs`.)

- **An error path that abandons an `McpClient` without calling `shutdown` still
  kills the server process.** A reviewer reads a bare `return Err(...)` in
  `connect`, `handshake` or `discover_tools`. Noting that
  `tokio::process::Child` does not kill on drop by default, they conclude the
  spawned server is orphaned. Codex flagged exactly this on the branch that
  made a failed `tools/list` fail the connect.

  `impl Drop for McpClient` calls `child.start_kill()`, so every abandoned
  client SIGKILLs its process, and tokio's orphan reaper collects it. That is
  the whole reason `shutdown` exists as a separate method: it is the *graceful*
  path, which also waits, not the only path that kills. Pinned by
  `a_failed_tools_list_is_a_start_failure_not_an_empty_server`, which blocks on
  the stub process actually exiting.

  Re-flag if the `Drop` impl is removed, or if a field is added that must be
  torn down in an order `Drop` does not give.
  (`crates/lucidos-engine/src/mcp/client.rs`.)

- **The rename shipped no alias, no doc line and no migration, and that is the
  decision.** `WorkingUnderstandingWritten` and `ContextKeptOpen` replaced
  `ScratchpadWritten` and `ContextKept`. Neither carries a `serde(alias)`, a
  `LEGACY_TYPE_NAME_ALIASES` entry, or a `Legacy alias:` line in
  `thread-events.md`. Codex read the missing reader as an upgrade dropping every
  thread's notes.

  There are no rows to read. The mode has only ever run in the maintainer's own
  eval, whose workspaces are disposable, so no workspace holds either row.
  Aliases for rows that do not exist are dead code.

  The `Legacy alias:` line went with the alias, on purpose.
  `.claude/rules/system-knowhow.md` requires that exact form for the
  workspace-audit **retired-event-name check**. Keeping it would claim the
  engine still reads the old name, which no code does. The cost is named and
  accepted: the audit cannot tell a dead `ScratchpadWritten` subscription from a
  workspace domain event.

  The renamed **preference key** is the same finding wearing a second face,
  and Codex flagged that one too. `self_curated_context_mode` reads nothing
  written under `context_mode_experimental`, the name it replaced, and there is
  no migration. There is no stored value to carry forward. Only the eval ever
  set it, and each run writes its own rows. The two schedule keys beside it are
  new, so they have no predecessor at all.

  Re-flag if the mode ships on by default, or if a workspace is found holding
  either row: a reader would then be back-compat rather than dead code. The
  same reasoning covers `ContextKeptOpen` and the retired keys.
  (`crates/lucidos-engine/src/engine/chat/process/working_understanding.rs`,
  `crates/lucidos-engine/src/core/preferences.rs`,
  `system-knowhow/thread-events.md`,
  `docs/plans/2026-08-24-self-curated-context-mode-engine-half.md`.)

- **The working understanding's `[TODO]` parse accepts `waiting` and
  `abandoned`, which `todo_write` refuses, and that asymmetry is the design.** A
  reviewer finds `todo_write_impl` rejecting both statuses from the LLM with a
  stated reason, then finds `parse_todo_line` mapping the same two words
  straight into `TodoItem.status`. It reads as a guard the new write path
  dropped. Two review angles raised it independently.

  The two paths are not the same act. A tool call is the model ASSERTING a
  status. The `[TODO]` heading is the model rewriting a list the engine just
  rendered TO it, and the render prints those two words on purpose: ADR 0109's
  amendment says re-entry is exactly when what became of an item matters most.
  Refusing the word would drop the whole line, so the item would vanish from
  the list rather than keep its state.

  The plan settles the syntax in as many words: the two engine-written statuses
  "take the same shape with a word in the bracket, so one rule covers all five
  marks". The residual is that a model can assert one fresh. It costs one round
  of a wrong mark in the prompt bar, and the next write corrects it, because the
  checklist is replace-whole.

  Re-flag with evidence that a model asserts one unprompted, or if the render
  ever stops echoing the engine's word: the round-trip is the whole reason the
  parse accepts it.
  (`crates/lucidos-engine/src/engine/chat/process/working_understanding.rs`,
  `crates/lucidos-engine/src/engine/tools/todo.rs`,
  `docs/plans/2026-08-24-self-curated-context-mode-engine-half.md`.)

- **`segment_heads` and `segment_heads_as_written` resolve the same token two
  different ways ON PURPOSE, and merging them reopens a bypass.** A reviewer
  reads the pair as copy-paste drift: one basenames the head, the sibling right
  beside it does not, and both walk the same segments. The obvious cleanup is
  to keep one.

  They sit on opposite sides of the grant lane. `segment_heads` DERIVES what an
  "Always allow" click stores, and basenaming is right there: `/usr/bin/git
  push` should store `git`. `segment_heads_as_written` is what a stored grant
  is MATCHED against, and basenaming is wrong there: it would let `Bash(ls:*)`
  cover `data/bin/ls`, a binary the agent writes in-workspace with an ordinary
  Safe write and then runs with no card. The Safe fast path refuses a
  path-qualified head for exactly that reason
  (`a_path_qualified_head_never_settles_safe`).

  The asymmetry has a visible cost, and it is not a bug either: a grant stored
  from `/usr/bin/aws …` covers a later bare `aws …` and not a later
  `/usr/bin/aws …`. The grant file header says so.

  Re-flag only with evidence that the matching side can basename safely.
  Pinned by `matching_reads_the_head_as_written_while_derivation_basenames_it`
  and `a_stored_grant_never_covers_a_path_qualified_head`.
  (`crates/lucidos-engine/src/engine/command_guard.rs`.)

## Desktop client (Tauri, macOS)

- **`unread_targets` returning `(Option<String>, String)` is a deliberate
  asymmetry, and collapsing it to two `Option`s reintroduces a shipped bug.** A
  reviewer will read the tuple as an inconsistency (both halves are "a label or
  nothing") and propose the tidier `(Option<String>, Option<String>)`. The two
  surfaces clear by opposite means. The Dock tile clears with AppKit's
  `setBadgeLabel(None)`, so `None` is the working call there. The menu-bar item
  clears only by being WRITTEN an empty string: `tray-icon`'s macOS backend
  (`set_title_inner`, 0.24.1) puts its `setTitle:` call inside `if let Some(..)`
  and does nothing on the `None` arm, so `tray.set_title(None)` updates the
  crate's cached `attrs.title` and leaves the old text on the status item
  forever. That shipped: an install whose unread count fell to zero read bell 0,
  Dock tile blank, menu bar frozen at 8. Both call sites carry the reasoning
  inline, and `a_cleared_tray_title_is_an_empty_string_the_tray_will_actually_write`
  pins it. Re-flag only with evidence that `tray-icon`'s macOS `set_title_inner`
  handles `None` by clearing the button title.
  (`lucidos-app/src/lib.rs` `unread_targets` / `apply_unread_indicator`,
  `lucidos-app/src/notifications.rs` `set_tray_title`.)

- **`calc(<length> / <number>)` is supported in the packaged WKWebView, and a
  claim that it is not is refuted by the header already shipping it.** A
  reviewer will see the halving term in the desktop header
  (`--header-band-lift`) and warn that WKWebView on
  the declared minimum (macOS 11, Safari 14) rejects division inside `calc()`,
  making the custom property invalid and silently discarding every rule that
  consumes it. It does not. Division by a unitless NUMBER is CSS Values Level 3
  and has been in Safari since 6; what arrived later is Values Level 4 TYPED
  arithmetic, dividing by a value that itself carries a unit
  (`calc(10px / 2px)`), which this codebase does not use anywhere. The decisive
  evidence is local rather than a spec date: `--header-band-lift:
  calc(var(--titlebar-inset, 0px) / 2)` is the load-bearing term for the whole
  band-centred header and has shipped in the packaged macOS app since that
  header landed, so were the construct invalid the bar would already be visibly
  broken on the only build it applies to. Re-flag only with a typed division
  (units on BOTH sides), or with an observed failure on a named OS version.
  (`lucidos-app/src/styles/panels/shell.css`, the desktop `:root` block.)

- **The unread badge loop recording `last` only after `run_on_main_thread`
  returns `Ok` is not redundant error handling.** A reviewer may read the
  `match` as ceremony around an infallible call and want the older
  `let _ = handle.run_on_main_thread(..)` back. Nothing reads the tray title or
  the Dock tile back, so `last` is the loop's ONLY account of what is on screen,
  and the `last != Some(total)` guard suppresses every rewrite of a value it
  believes it already applied. Banking a hop that was never queued therefore
  pins both surfaces at a stale count permanently rather than for one tick.
  Re-flag only if something starts reading the applied value back.
  (`lucidos-app/src/desktop.rs`, the macOS unread-indicator thread in `launch`.)

- **The function-key text guard runs unconditionally on purpose, and gating it
  on Tauri does not buy what a reviewer expects.** `installNoFunctionKeyText`
  (`utils/noFunctionKeyText.ts`) cancels a `beforeinput` whose data is entirely
  AppKit function-key codepoints, and it installs in the web build too. Codex
  flagged that twice in one hardening run, once against the original
  0xF700-0xF8FF bound and again after the narrowing: a browser client can no
  longer type a private-use glyph in the range through an IME.

  The mechanism is real and the trigger is remote. The insertion has to be
  ENTIRELY function-key codepoints, and a paste puts its content on
  `dataTransfer` and leaves `data` null, so pasting one is untouched. What is
  left is an IME or a Character Viewer committing a lone glyph in a 72-codepoint
  window Apple assigns to keys.

  The gate is not the remedy for that. A Character Viewer glyph is likeliest on
  macOS, so a Tauri-only guard keeps the collateral where it hurts and drops the
  protection everywhere else. Whether the fallthrough is Tauri-only was never
  verified either, and the same WebKit lives in the iOS PWA. Narrowing the bound
  to the last assigned constant was the real answer, and it is already applied.
  Re-flag only with a client that demonstrably types such a glyph, or with
  evidence the fallthrough cannot reach a browser.

- **The native cursor mirror needs no blur reset, and a blur reset would be
  worse than the gap it closes.** A reviewer reading `utils/nativeCursor.ts`
  observes that a Cmd-Tab away fires no `pointerout`, so a `col-resize` ask can
  outlive the hover. Codex flagged it as P2 on the branch that added the file.

  An `NSWindow` cursor rect governs that window alone, so nothing our window
  asked for can reach another application's window. Whatever is under the
  pointer over there sets its own. Coming back cannot land the pointer on a
  different element either, because a Cmd-Tab moves no pointer, so the ask is
  still the right one.

  The proposed remedy introduces the wrong state it was meant to prevent. On
  blur the pointer is still over the divider. Resetting to the arrow therefore
  makes the glyph wrong the moment focus returns, until the user moves a pixel.
  Every case a blur listener could catch is one where a pointer move follows,
  and a move re-resolves the cursor on its own. Re-flag with a path that leaves
  the window's cursor wrong AFTER a pointer move.

## Frontend

- **`answerableQuestionId` reads the DOM rather than the thread projection, and
  the staleness a reviewer sees is not removable by reading the store instead.**
  `scrollState.ts` decides whether a send is really an answer by asking the
  newest `.question-body`. Two reviewers flagged the same shape on the branch
  that added it: use the send path's thread state, and drop
  `question-body-terminated` from `DEAD_QUESTION_CLASSES` because a terminated
  body carries no id anyway.

  The DOM is not the weaker source here. `scrollState` may not import `store` at
  all (see `parseNavigatedTurn`), and for a MULTI-SELECT answer the DOM is
  strictly fresher: `setPendingAnswer` swaps in `AnsweredBody` on the tap, while
  `effectiveThreadStatus` waits for SSE. For a typed answer neither knows until
  the event lands, so moving the read to the store keeps the same window.
  The window costs a landing anchored on the card instead of the new row, and
  both rest at the live edge (ADR 0080).

  The terminated entry is belt to a brace on purpose, matching `turnIsQueued`
  in the same module: recognise a state POSITIVELY, never by an absent
  attribute. An id added to `TerminatedQuestionBody` would otherwise make a dead
  card answerable with nothing failing. Re-flag only if the module gains a
  legitimate `store` import, or if the card stops carrying its state in a class.

- **`tabIndex={-1}` on a toast or confirm-dialog scroll box takes away no
  keyboard scrolling, because that surface's own Tab cycle never reached it.**
  Chrome promotes an overflowing scroller to a Tab stop when it holds no
  focusable child. A reviewer therefore reads the attribute as removing what
  Chrome just granted. Codex flagged both boxes as P1 on the branch that added
  them.

  Neither was reachable. `handleToastKeyDown` (`components/shared/Toast.tsx`)
  intercepts every Tab inside a toast that owns a button, and cycles
  `a[href], button` only. `trapDialogTab` wraps at the confirm dialog's two
  buttons via `trapTargetIndex`, which acts at the boundaries and answers null
  in between. `.confirm-details` sits before both in DOM order, so native
  movement only ever steps Cancel to OK. A plain scroller is not click-focusable
  either. The attribute states in the DOM what the trap already enforced, and
  stops the promotion racing it.

  The file preview modal is the counter-case and is written the other way:
  nothing traps Tab there, so `.file-preview-modal-body` declares
  `tabIndex={0}` with a role, a label and the shared `--focus-ring`. Re-flag
  only if a toast or the confirm dialog grows a Tab cycle that includes its
  scroll box.

- **`canInstallUpdateHere` subtracts `installer-rerun` rather than requiring
  `desktop-app`, and that asymmetry is deliberate.** A reviewer reads the
  release check's `install` field as "what can act on this offer". They then
  propose `install === 'desktop-app'`, so an unrecognised layout
  (`install: null`) offers no action. Codex flagged exactly this on the branch
  that added the field (ADR 0108).

  The field describes the **gateway's own** executable layout, not the client's.
  `isTauri()` is the conjunct that answers "can this session install", and it is
  already required. A Tauri client is a `.app` by construction and always has a
  working updater. So `install` is only ever used to SUBTRACT the one case where
  a Tauri session must not reach for it: a gateway that is a headless install.

  Requiring `desktop-app` would withhold a working button in two live states. A
  bundle whose layout `install_shape` stops recognising would show an offer it
  cannot take, silently. And a Tauri DEV client has no `latest` at all
  (`supported: false`), so `install` is `undefined` there and the dev
  engine-health update affordance would disappear. Re-flag only if a Tauri
  client can front a gateway it did not spawn.

- **A clickable toast whose handler is idempotent needs no `acted` guard, and
  the two that carry one say why in the comment.** The `BackupFailed` toast's
  `onClick` (`store/actions/thread-sync.ts`) dismisses and navigates with no
  re-entry flag. Two siblings in `store/actions/in-app-notification-toast.ts`
  set `acted` / `opened` first, so this reads as a missed guard. Two of three
  hardening angles flagged it on the branch that added it.

  The guard's own comment states its precondition: Toast.tsx fires `onClick`
  raw and the DOM lingers across the async dismiss render, so a double-tap must
  not re-run a **non-idempotent** open (`openAppById`,
  `focusThreadOrBootstrap`). A settings deep link is not one. `removeToast`
  reassigns only while the key is present. `pushEntry` returns `null` on
  `statesEqual` with the entry at the cursor. `revealContentPane` twice is a
  no-op. So the second tap changes nothing.

  Re-flag when the handler gains a side effect that is not idempotent: a POST,
  a counter, an id-minting open. Not for the shape alone.

- **The system-theme watcher registers its own resume listeners instead of
  calling `onPageWake`, because it needs the case `onPageWake` drops.**
  `preferences.ts` binds `visibilitychange` / `focus` / `pageshow` by hand.
  `utils/pageVisit.ts` already exports a coalesced subscription over that exact
  set, so this reads as a missed reuse.

  `onPageWake` fires only when a hide PRECEDED it: `comeBack()` returns early
  unless `away`, and `away` is set only by `pagehide` or a `hidden`
  `visibilitychange`. A window that merely lost focus never went hidden. A Mac
  that slept through an OS appearance flip hears nothing on the way back.
  Repairing that is half of why the watcher exists (ADR 0092). The iOS-only
  `onPageResume` is a worse fit again: it also arms a click-swallow.

  Re-flag only if `onPageWake` starts firing on a bare focus, or if the theme
  watcher stops needing the desktop case.

- **The iOS repaint toggle's `entry.restoreTop!` is guarded by the
  `nudgedTop !== undefined` test beside it, in both readers.** A reviewer sees a
  bare non-null assertion in `utils/iosRepaint.ts` and asks what happens when
  `restoreTop` is unset. One synchronous block in rAF1 writes both fields, and
  nothing else writes either. So `nudgedTop` being set is exactly the proof
  `restoreTop` is. Both readers ask that question first: `restoreNudge` guards
  its restore on it, and `nudgedTransform` returns a zero delta without touching
  `restoreTop`. Re-flag only if a second site starts setting `nudgedTop`, which
  would break the pairing.

- **A browser clamping the nudge write is a pre-existing yield, not a drift.**
  Fractional layout can leave a container scrollable by less than a pixel. The
  write then lands short, and `restoreNudge` finds a value it did not write. It
  DECLINES, by design: it never clobbers a concurrent writer, and it cannot tell
  a clamp from the reader.

  A reviewer reads that decline as a stranded offset. The alternative, reading
  `scrollTop` back to learn the true delta, costs a forced layout between the
  two writes for a sub-pixel case. Re-flag only with a report of a container
  left visibly off.

- **A new deep-link claim clearing `_pendingEventScrollLandedOffEdge` cannot
  resurrect a ride, because the thread's RECORD is the durable guard.** The flag
  (`components/chat/scrollState.ts`) says the last landing rested off the live
  edge, and it resets when a claim begins. A reviewer correctly observes what
  that costs. A second link claiming right after a first landed off the edge
  clears the one piece of state saying that landing ended the ride. Codex
  flagged exactly this, twice, on the branch that added the flag.

  The in-place resume never runs on that state alone. Both callers read the
  thread's *reading position* first: `standDownForDeepLink` resumes only for
  `recorded?.kind === 'live-edge'`, and `onPageWake` returns early otherwise. An
  off-edge landing records an OFFSET for that thread (`recordDeepLinkLanding`),
  so a beat later there is no live-edge record to resume at all. The *follow
  seed*'s own branch (`recorded === null`) reaches the same resume, and the same
  landing gives that thread a record too.

  What is left is the save's own debounce, a couple of hundred milliseconds in
  which the record still reads live-edge. Reaching it needs a second link
  claimed AND an attach or page wake inside that window. Widening the flag's
  lifetime does not close it either, since `releaseClaim` clears the flag on the
  same path. Re-flag only if the resume stops reading the record first, or if
  the landing stops recording an offset.

- **The derived live `Thinking` row carrying no `created` is deliberate, and no
  consumer reads a missing one.** Every other `pushStep` in
  `store/thread-events/exchange-render.ts` stamps the source event's `created`,
  so the one in the `needsLiveThinkingRow` branch reads as an omission that
  will render an invalid timestamp. It cannot: the row is DERIVED from the
  turn's live state and corresponds to no event, so there is no honest
  timestamp to stamp (`frontend.md` § No Silent Defaults), and both consumers
  handle its absence, `InlineStep` because it never reads `created` at all and
  `StepDetailModal` because its timestamp is behind `{step.created && …}`. The
  field is optional on the `ResponseEvent` step variant for this reason.
  Re-flag only if a consumer starts reading `created` unguarded, or if the row
  acquires an event of its own. (`store/thread-events/exchange-render.ts`,
  `components/chat/chat-exchange-parts.tsx`, `components/chat/StepDetailModal.tsx`.)

- **The shared `Dropdown` portals its menu to `<body>` at a z-index BELOW
  `--z-modal`, and no dropdown is nested in a modal, so the "hidden behind the
  modal it belongs to" bug has no instance.** The shape invites the finding
  (Codex raised it as P1 on 2026-08-09): a body-portaled panel at
  `--z-control-panel + 1` would indeed render under a `.modal-overlay`, and its
  clicks would read as "outside" to the enclosing overlay's dismiss handler.
  What refutes it is that **no `<Dropdown>` renders inside a backdrop
  `<Overlay>`**. `CredentialModal` and `TriggerDetails` are the two that look
  like modals and are not: both are reached through `InlineForm`, which
  `ContentPane` renders as an ordinary content-pane view inside
  `.content-pane-body` (`overlay?.type === 'form'`), which is why neither file
  contains an `<Overlay>` at all. The name is a leftover. Every other
  Dropdown host is a settings page, the composer, or a content-pane view.
  Re-flag only with an actual nesting: a `<Dropdown>` rendered inside a panel
  that is itself a backdrop `<Overlay>`. The `.dropdown-menu` rule in
  `styles/global/host-components.css` states the ordering constraint at the
  site, including what to do if that day comes.

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
- **RETIRED 2026-08-08 (the deep-link deadline's recovery scroll).** This prior
  defended a `watchUserAction`-gated scroll to the newest turn when a deep-link's
  target never rendered. Both the scroll and its gate are gone: the transcript no
  longer moves on the app's own initiative at all, so a dead link reports through
  `onUnresolved` and leaves the reader where they are. Kept as a row rather than
  deleted, because a reviewer reading `ad48eadad` (which removed an
  unconditional snap) or `2026-08-05`'s recovery (which added a gated one) is
  reading two live-looking commits about a behaviour that no longer exists. The
  current rule is in `docs/glossary.md` § Reading position and
  § Navigation scroll. Flag a NEW recovery scroll here as a regression.
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
  transform:translate(-50%,-50%)` plus a symmetric `max-width` reserve, so it
  sits on the viewport/row axis (like the pane dots + desktop header) regardless
  of the leading (thread-drawer toggle or hamburger + nav, or filter) and
  trailing (actions) cluster widths. That reserve is DERIVED from
  `--header-nav-cluster-width`, not the `calc(100% - 10.5rem)` constant this
  entry used to quote: the threads row's Lucidos mark sits on the nav cluster's
  trailing edge now, and a rem constant agrees with a clamped cluster at exactly
  one ui-scale (they kissed at 150%). Do not "restore" the constant. The
  requirement was stated as *"they should be centered, as long as they don't
  overlap the left-side icons; if centering would overlap, move them right so the
  left edge clears the rightmost left icon."* The symmetric reserve delivers both:
  a short title reads centered; a long one clamps + ellipsizes with its left edge
  just past whatever the reserve is sized to clear. That used to be the widest
  leading cluster at ~5rem; on the one row still using this class it is the
  Lucidos mark on the nav cluster's edge, which is further in. An in-flow
  flanking-spacer title
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

- **An OAuth flow recorded with `provider: null` deliberately matches any
  completion.** `handleOAuthAccountConnected`
  (`store/actions/oauth.ts`) fronts the window only when `oauthAuthFlow` names
  the provider the event carries, OR when it names no provider at all, and that
  second arm reads like a hole in the check. It is the only thing the agent path
  can say: the engine's `NavigationRequested` for an authorization
  (`engine/tools/credentials.rs`) carries `purpose: "oauth"` and a URL, no
  provider, so the page opening it does not know which provider it is
  authorizing. Plumbing one through would not close the gap either, because the
  navigate is scoped to the focused thread and two windows viewing that thread
  would record the SAME provider. The distinguisher would have to be per-window,
  which the event has no notion of. The residual is bounded: a stale marker from
  an abandoned agent-started flow fronts its own window once, in the same
  workspace, right after the user's own OAuth action, and never creates or
  unhides a window. Re-flag only if the navigate payload gains a per-window
  identity to key on. (`store/actions/oauth.ts`,
  `crates/lucidos-engine/src/engine/tools/credentials.rs`.)

- **The thread filter panel's "Include deleted" checkbox sits INSIDE the
  radiogroup on purpose** (2026-08-09). ARIA says a `radiogroup` owns radios,
  so a checkbox and its Explainer partway through the set reads as a
  conformance break, and both Codex and a fresh reviewer will reach for it. It
  is the deliberate resolution of a three-way trade-off. The panel is ONE
  single-select set (four statuses, an "or" rule, then "All statuses"), and the
  requested reading order puts the modifier directly under that last row so the
  "By thread types" heading below lands on the thread types it names. Splitting
  the wrapper into two radiogroups to keep the
  checkbox out reports one choice as two independent sets, which misleads worse
  than a foreign child does; CSS `order` keeps the DOM clean but desynchronises
  tab order from the visuals, which is a real keyboard bug rather than a
  conformance one. The rows are `<button role="radio">` with no roving
  tabindex, so the practical AT behaviour is per-row announcement plus Tab,
  which the interleaved checkbox does not disturb. Re-flag only with a fix that
  keeps ONE group, the DOM order, and the visual order at once.
  (`components/layout/ThreadFilterPanel.tsx`.)

- **"Include deleted" being off DOES count toward the "filtered" note, by
  product decision** (2026-08-09). The mechanical objection is correct and will
  be re-derived every round: `deletedOptionsHidden` gates which triggers / repos
  / apps the OPTION LISTS offer, and excludes no thread from the list on screen,
  so folding it into `narrowed` reports a narrowing the thread list does not
  have. It was raised with the user before it was built and overruled twice, in
  those words: a workspace where something deleted is being held back is not
  showing you all of it, and the row says so. The refinement that answers the
  mechanical objection came from the same place and is what `deletedOptionsHidden`
  encodes: the note fires only when something deleted actually EXISTS and is
  unselected, never on the switch merely sitting at its default, so the default
  view on a workspace that has never deleted anything stays quiet. The explainer
  inside the note names this cause in its own paragraph, so the user is never
  left guessing which setting is responsible. Re-flag only if the note starts
  firing with nothing deleted, or if "Include deleted" gains the power to hide
  threads (at which point the two causes stop being distinguishable and the
  copy needs revisiting). (`components/layout/ThreadFilterPanel.tsx`,
  `store/threadFilterActive.ts`.)

- **There is no hidden duplicate transcript, so no "the hidden mount steals the
  shared scroll state" bug.** `App.tsx` mounts ONE layout tree, `SplitLayout`
  or `MobileSwipeContainer`, gated on `viewportIsMobile`; each renders
  `ThreadPane` once, and `ThreadPane` renders `<ThreadView/>` or
  `<CreateThreadView/>` from one ternary. So `.thread-content` exists once, and
  a finding of the shape "on mobile both the visible and hidden transcript
  attach, and if the hidden one runs last it claims the global" is describing a
  mount that was removed (dual-mounting fanned every signal write out to two
  subtrees). Two things keep inviting it and neither is evidence: the
  `isElementVisible` gates, which are real but exist for containers laid out at
  0x0 (a collapsed desktop split, mobile's zero-height `.content-row`), and the
  phrase "the hidden dual-mount copy" still used around this file as shorthand
  for an element with no box. Re-flag only if a second simultaneous mount is
  reintroduced in `App.tsx`, which the header of `scrollState.ts` now states as
  the premise to check. (`components/chat/scrollState.ts`, `App.tsx`,
  `components/layout/ThreadPane.tsx`; raised by the Codex reviewer on
  2026-08-09 against the live-edge reading position.)

- **`AppHeader`'s local `isInteractive` is NOT a duplicate of `utils/dom.ts`'s
  `isInteractiveTarget`, and consolidating them would change behavior in both
  directions** (2026-08-10, the workspace picker's window drag region). The two
  read as copy-paste and a reviewer will reach for the DRY rule, especially once
  a second drag region imports the shared one. Compare the selectors before
  doing it. The header's carries two header-only surfaces the shared one has no
  business knowing about, `.hamburger-panel` and `.thread-toggle`, so switching
  would make both draggable and break the gate they exist for; and the shared
  one carries `textarea`, `label`, `[role="button"]` and `[contenteditable]`,
  which the header would newly exempt. A drag gate is a per-surface policy, not
  one predicate: the picker wants the broad DOM answer because its whole
  background is the region, the header wants its own list because it is a strip
  of named controls. The DRY rule binds "when you touch a file" anyway, and a
  diff that adds a caller of the shared helper elsewhere has not touched
  `AppHeader.tsx`. Re-flag only with the two selector lists shown to be equal.
  (`components/layout/AppHeader.tsx`, `utils/dom.ts`,
  `components/picker/WorkspacePicker.tsx`.)

- **`followSentMessage` deciding BEFORE the optimistic row renders is the
  point, not a timing bug** (2026-08-10, the brand-new-thread pin). It asks
  `hasLiveEdgeToRide` about the transcript as it stands at send time, and both
  call sites (`PromptInput.submit`, `addPendingMessage`) run before the row is
  in the DOM, deliberately: `addPendingMessage` calls before its own
  `threadMap` write for exactly this reason. A reviewer reasonably observes
  that a thread which currently fits can be pushed past the fold by the new
  message itself (a long paste, or a brand-new thread's long first message),
  so nothing arms and the tail of what they just wrote sits below the viewport
  (Codex flagged this on the commit that added the rule). The observation is
  accurate and the proposed fix, deferring the decision until the row exists,
  re-creates the reported bug: a brand-new thread then reads as "one turn, and
  scrollable", arms, and drags the reader down through the first reply, which
  is the entire complaint. Leaving them at the top of their own long message is
  the intended outcome: they wrote it, they read down through it, and the
  chevron is one tap. Re-flag only with evidence that the two call sites moved
  after the render, or that the rule itself changed.
  (`components/chat/scrollState.ts`, `store/actions/chat.ts`.)

- **A superseded deep-link's late resolve retiring the standing follow is
  consistent with the landing it is attached to** (2026-08-10, same commit).
  `tryResolve` runs `stopFollowingBottom()` on the line above
  `smoothScrollToElement`, unconditionally, while the resolve BROADCAST below
  it is gated on `_pendingEventScrollClaim === claim`. A reviewer reasonably
  proposes gating the retirement the same way, since a first link superseded by
  a second keeps observing until its own deadline and can resolve late. Gating
  only the retirement makes it worse, not better: the stale call scrolls the
  container either way (the pin, the scroll and the pulse are all ungated by
  design, so a link that finally finds its target still lands), and a reader who
  has just been moved must not be left following. The two belong on the same
  line so they cannot disagree. Re-flag only if the late landing itself is made
  to stand down, in which case the retirement goes with it.
  (`components/chat/scrollState.ts`.)

- **`reachableScrollTop` rounding the anchor correction does NOT throw away
  subpixel precision the engines would have kept** (2026-08-11, Codex).
  `withScrollAnchor` (`components/chat/CreateThreadView.tsx`) measures the
  anchor through rects, in doubles, and then rounds the one number it writes,
  which reads as undoing the precision the measurement just won: `scrollTop` is
  a double on the way in and out, so an unconditional `Math.round` looks like up
  to half a pixel of self-inflicted drift. The engines do not keep the fraction.
  Measured on a seeded 12-turn transcript at a 105% root: WebKit stored **2377**
  for a written **2377.8** and the reader moved 0.8px, and Chromium stored
  **2499** for **2498.8**, at both device pixel ratios. WebKit TRUNCATES where
  Chromium rounds, so handing the engines the fraction is not neutral, it hands
  iOS up to twice the desktop's error; rounding first took the worst press in
  that run from 0.75px to 0.39px. The residual under half a pixel is the floor
  for any anchor correction, because layout is fractional and a scroll offset is
  not. Re-flag only with a measurement showing an engine that stores a
  fractional `scrollTop` it was handed.
  (`components/chat/CreateThreadView.tsx`,
  `e2e/turn-control-holds-the-reader-still.spec.ts`.)

- **Multiplication inside `calc()`, custom properties included, is not an
  unsupported-browser risk here** (2026-08-12, Codex). The context viewer's
  indent ladder computes a row's inset as `calc(var(--context-row-inset) +
  var(--context-depth) * var(--context-indent))` (`styles/steps.css`), where the
  depth substitutes a bare number, and a reviewer flagged the whole declaration
  as dropped on older WebKit, taking the tree's hierarchy with it. The premise
  is wrong twice over. It is CSS Values 3, in Safari since well before any
  macOS this app runs on, and Playwright's WebKit resolves the ladder to
  8/24/24/40px exactly as Chromium does. More decisively, the app cannot run at
  all on an engine that lacks it: EVERY `--duration-*` token is
  `calc(<literal> * var(--duration-scale))` (`styles/global/base.css`, pinned by
  `styles/__tests__/duration-scale-guard.test.ts`), so every transition in the
  app would be dead, and the desktop split's divider is
  `calc(var(--co) + var(--ddo) + var(--sr) * (100% - var(--co) - var(--ddo)))`
  (`styles/panels/shell.css`), a unitless var times a parenthesized expression,
  which is strictly more than the ladder asks for. Re-flag only with a named
  engine, in the supported set, measured dropping one of these declarations.

- **One shared toast column is the chosen design, not a revert of the per-pane
  fix.** `34098b4c2` gave each pane its own `.toast-column`, so a toast raised
  in one pane could not push the other pane's toasts down. A reviewer who finds
  every toast back in a single stack will read that as the same bug returning.

  It is a supersession, decided with the user and recorded in
  `docs/plans/2026-08-13-toast-banner-dialog-taxonomy.md` § Settled Decisions.
  The old bug is unreachable rather than tolerated: it needed two panes each
  holding their own toasts, and there is now one stack for all of them. Origin
  moved off the axis entirely, and each message goes to the surface matching its
  weight instead. Per-pane survives as one option in the placement picker while
  the shape is being chosen (`docs/temporary-measures.md`).

  Re-flag only if per-pane columns come back AND a toast in one pane again
  displaces the other's.

- **An optional chain on state a guard already narrowed is load-bearing when
  the reader is a hoisted function.** A component early-returns on
  `devices === null`, so `devices.length` reads as safe below it. Inside a
  nested `function` declaration it is not: the declaration is hoisted above the
  guard, so TypeScript resets the narrowing and `devices.length` is
  `TS18047: possibly 'null'`.

  So `(devices?.length ?? 0)` in such a body is not dead defensiveness, and
  "simplifying" it fails the type check. This was flagged and reverted during
  the hardening of `PairedDevicesSection`'s revoke confirm. The narrowing holds
  in JSX and in an arrow function assigned after the guard, which is why it
  looks inconsistent.

  Re-flag only if the reader stops being a hoisted declaration, or the state
  stops being nullable. (`crates/lucidos-app/src/components/settings/PairedDevicesSection.tsx`.)

- **The header-control wash is written three times on purpose, and hoisting it
  to a token on the bar breaks it.** A reviewer sees
  `background: color-mix(in srgb, var(--header-fg) var(--header-control-veil), transparent)`
  in three rules of `styles/panels/shell.css` (a header `.icon-btn` hovered,
  toggled on, and both at once) and proposes one `--header-control-bg` on
  `.pane-header`. It cannot work. The browser substitutes a `var()` inside a
  custom property on the element that DECLARES that property. So the hoisted
  copy resolves the veil against the bar's resting `0%`, and every state
  inherits a transparent wash.

  The alpha is the only thing the states vary, so the wash belongs where the
  alpha is declared, on the same element. The same fact is what makes the badge
  ring work: `.app-header .badge` mixes the veil ON THE BADGE, so it reads the
  veil the control under it raised.

  Re-flag only with a shape that keeps the substitution on the state element.
  (`crates/lucidos-app/src/styles/panels/shell.css`, pinned by
  `styles/__tests__/header-badge-ring.test.ts`.)

- **The prose path matcher stopping at `?` or `#` is deliberate, not a
  truncation bug** (2026-08-21). `artifacts/report.html?v=2` links only
  `artifacts/report.html`, leaving `?v=2` as text. A reviewer reads that as an
  extension cut short at punctuation, and Codex flagged it.

  It is what the cached-list matcher already did with a query string.
  `extractDataPathTarget` strips a query and a fragment anyway, so the
  `data-path` is identical either way. `FILENAME_CONTINUES` therefore omits `?`
  and `#` on purpose while rejecting `-_~+%`, which really would continue the
  filename.

  Re-flag only if `extractDataPathTarget` starts preserving a query string.
  (`crates/lucidos-app/src/utils/linkifyPaths.ts`.)

- **A hyphenated word before a sub-tree name can match, and is left alone**
  (2026-08-21). The prose boundary accepts `-`, so `foo-knowhow/a.md` offers
  `knowhow/a.md` as a candidate. It looks like the `system-knowhow` prefix
  being sliced in half.

  `system-knowhow` itself is safe by construction: its match starts earlier, and
  earliest-start wins in `resolveMatches`, so the inner `knowhow` is never
  reached. What is left needs an English word hyphenated onto a sub-tree name
  and a real filename after it. Rejecting `-` as a boundary would cost more than
  that case is worth.

  Re-flag with a case that reads as ordinary agent prose.
  (`crates/lucidos-app/src/utils/linkifyPaths.ts`.)

- **The waiting indicator naming MORE sub-threads than `activeChildrenCount` is
  the honest direction, not an off-by-one.** `activeSubThreads`
  (`components/chat/WaitingPanel.tsx`) resolves rows from `threadMap` and
  subtracts only in one direction: a shortfall becomes an "and N more" row, and
  a surplus is listed. Codex flagged the surplus as P2 on the branch that added
  the panel, asking for the rows to be capped to the server's count.

  A capped list would hide a child the rest of the UI draws as running, since
  the drawer row reads the same `effectiveThreadStatus` this does. The cut would
  also land on the newest child, which is the one that just started and the
  reason the count is briefly behind. Every listed row is a real sub-thread of
  this parent, mid-turn by the same predicate the engine uses
  (`active_thread_statuses()`).

  The asymmetry with the `count <= 0` early return is deliberate and is the
  performance gate: the count decides WHETHER the thread is waiting on children,
  the map only names them, and the prompt row re-renders on every `threadMap`
  flush. Re-flag if a caller starts treating the row list as a counter, or if
  the count becomes the fresher of the two.
  (`crates/lucidos-app/src/components/chat/WaitingPanel.tsx`.)

- **An e2e `afterEach` that restores global workspace state asserts on purpose,
  even though a failed restore then reds a test whose body passed.** A reviewer
  reads the `enableMobileHeaderSticky` teardown in the four specs that turn the
  mobile header pin off. Seeing the helper's own `expect(res.ok())`, they
  propose a quiet restore, so teardown can neither mask nor invent a verdict.

  Quiet is the worse failure. `mobile_header_sticky` is a GLOBAL preference, and
  the e2e database resets only between projects. A restore that fails in silence
  hands live hide-on-scroll to every later spec in the project.

  The 2026-08-24 nightly is what that costs: `trigger-groups` lost its Save. The
  mousedown landed on the button and the mouseup on the wrapper, after the
  header shifted the form. That reads as a product bug in a spec which never
  touched the preference. A loud teardown names the real fault instead.

  Re-flag if the preference becomes per-context rather than global, or if the
  suite starts resetting the database between specs. Either would make a failed
  restore harmless.
  (`crates/lucidos-app/e2e/helpers.ts`.)

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

  **Python docstrings are the one exception, and they meet that second bar.** A
  reviewer seeing `in_pydoc` carry across lines reads it as the reset above
  being violated. It is the exemption the entry invites: a docstring IS the
  comment in Python, so per-line reset does not misread a few lines, it books
  every docstring as code. The tracker is bounded the way the entry asks. It
  arms only for `.py`, matches an unescaped delimiter, and an unterminated run
  at end of file warns and exits non-zero. Re-flag only for a shape the
  fixtures miss, naming it.

  **The `has_comment = 1` at the head of that branch does NOT capture blank
  lines**, and the same reading is available against `in_block` above it. Both
  branches sit inside `while (length(s) > 0)`, which an empty line never
  enters, so `classify()` returns with both flags clear and the caller books
  the line blank. That is cloc's partition: a blank line inside a comment block
  is blank. The `python: docstrings are comments` fixture pins it with a blank
  line inside a module docstring. Read the loop guard before flagging this one.

- **`sloc.awk` decides Python docstring position from ONE line of memory, and
  bracket depth is the wrong upgrade.** A reviewer sees `py_prev_cont` remember
  only the previous line's last character, builds the case
  `QUERY = (` / `"select "` / `"""from table"""`, and proposes bracket depth
  instead.

  The gap is real and measures near zero. Count a bare string-literal line
  followed by a line-head triple quote: 0 hits in a 2,483-opener repo, 7 in a
  48,734-opener one, 0 in a third.

  The remedy costs far more than 0.01%. Depth counting must skip brackets inside
  strings and comments, which needs the cross-line string state the entry above
  rejects. Getting that wrong drifts silently and forever.

  That is not hypothetical. The throwaway depth counter written to size this
  residual reported 334 hits. Its first three examples were `def` signatures
  whose next line held an ordinary docstring, because brackets inside SQL
  strings had pushed the depth positive. One line of memory can mislabel one
  run. A drifting depth mislabels every run after it.

  Re-flag only with a corpus where the concatenation shape is material.
  (`.claude/skills/project-stats/sloc.awk`, `py_prev_cont`.)

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

- **The e2e lock's emit capture records an empty `$LUCIDOS_WORKSPACE` as
  ABSENT, and that cannot address an event anywhere the live value would not**
  (2026-08-09, `_e2e_capture_emit_env`). The tempting flag is precise about the
  CLI and wrong about the consequence: `resolve_from_env` really does
  `env::var(..).ok()`, so `Some("")` is distinct from `None`, and the capture
  really does collapse the two. What the two forms then DO is the part to check.
  `Some("")` joins `.lucidos/ports` onto an empty root, giving a path relative to
  the process cwd, with no fallback if it is missing. `None` walks up for the
  same file starting AT that same cwd (`walk_up_for_ports` tests `start_dir`
  before its parents). So wherever the empty form resolves at all, the walk-up
  resolves to the identical directory on its first iteration, and wherever the
  empty form errors, it addressed nothing to diverge from. The capture pins the
  cwd alongside the workspace precisely so this holds at both ends of a hold.
  Re-flag only if the CLI stops starting its walk-up at the cwd, or if something
  begins setting `LUCIDOS_WORKSPACE` to the empty string deliberately.
  (`scripts/lib/e2e_lock.sh`, `crates/lucidos-cli/src/workspace.rs`.)

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

- **BOTH bodies carry an explicit `font-size: var(--font-size-md)` now, and so do
  form controls. Deleting either one to "let it inherit" is a bug that has
  shipped three times.** This entry used to say the opposite for the host, on
  the theory that every host text element names its own token, so the host
  body's computed size was a value nothing rendered at. It carried a re-flag
  clause, *"re-flag only if the host shell starts rendering real body text at the
  unstyled root size"*, and on 2026-08-12 that clause fired: Settings > System >
  What's New shipped release notes larger than the version heading above them,
  because `.whats-new-notes` styled its padding and stopped. The theory required
  100% coverage forever, and one miss renders visibly wrong, so the host was
  given the same default as the iframe on 2026-08-13.

  **Why the failure is always "too big".** The root is
  `var(--user-ui-scale)`, which is exactly `--font-size-xl` (`1rem`, labelled
  "section headings"), while body is `--font-size-md` (`0.8125rem`). Text that
  reaches the root is therefore a step and a half ABOVE prose, not "unstyled".
  Reviewers reading a too-large surface should look for a MISSING declaration
  before looking for a wrong one.

  **Three separate deletions, so the history is the point of this entry.**
  `2a742266b` (2026-06-19) dropped the iframe's `body { font-size: 0.875rem }`
  reasoning that it "made app text permanently smaller and the user's UI-scale
  preference look ignored". Both halves were wrong: `0.875rem` is a `rem`, so it
  tracked the root the whole time, and 14px sat at the TOP of the host's range
  rather than below it. That was measured against the host body's *computed*
  size instead of against any pixel the host paints. Restored as
  `--font-size-md` on 2026-08-05 after a user reported apps rendering a scale
  step larger than their threads. The host's own absence caused What's New, and
  the control gap caused `.welcome-dismiss` to paint in Arial while sizing
  itself correctly from a token.

  **Controls are a separate default, not the same one.** A control inherits
  nothing from `body`: the UA stylesheet applies the `font` shorthand to it.
  `base.css` hands the family and size back with LONGHANDS on
  `input, textarea, select, button`, and the shorthand must never be used there,
  because it also resets `font-weight` and `font-feature-settings` (which would
  give Fira Code's ligatures back to every control at once). `html` is
  deliberately absent from that selector list: `font-size: inherit` on the root
  would override the ui-scale declaration, both being element selectors.

  Pinned by `api::sdk::tests::iframe_body_is_sized_from_the_type_scale`,
  `styles/__tests__/text-defaults-guard.test.ts`, and the rendered
  `e2e/type-scale.spec.ts`. Re-flag if any of the three defaults goes missing,
  or if a `font` shorthand appears on the control rule.
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

- **`hasRenderableResponseContent` counts a `step` as drawn even though
  `renderResponseEvents` gates it on `showSteps`.** Reviews read that as an
  incomplete mirror: with steps collapsed the renderer returns `null` for them,
  so a boundary holding only a step would supposedly still open an empty panel.
  It does not. `getEventToggleState`'s `showStepsToggle` is the same predicate
  (`some(isStepMechanics)`), so a step present means the body always renders the
  "Show steps" button: visible content, and the affordance that reveals the
  rest. The predicate is about whether a panel is worth opening, not about which
  of its rows are currently expanded, and threading `showSteps` into it would
  make an exchange's panel appear and disappear as the user toggles a global
  preference. The narrower shape the concern usually reaches for, a lone
  `CodingAgentToolResult`, produces no step at all: that arm only resolves a
  pending step, it never pushes one. Re-flag only if `showStepsToggle` stops
  keying on the same predicate, which would make the button genuinely absent.
  An `event_wait` needs none of this reasoning since 2026-08-08: it is a
  *transcript marker*, not step mechanics, so no toggle can hide it and it is
  drawn whenever it is present.
  (`crates/lucidos-app/src/store/thread-events/exchange-render.ts`,
  `store/event-rendering.ts` `getEventToggleState`.)

- **The event-wait routes bind the caller's thread only when the caller HAS
  one.** `refuse_event_waits_for_another_thread` refuses a request whose
  thread-bound origin token names a different thread than the path, and lets an
  UNTOKENED request through. Reviews read the second half as the check being
  incomplete: any local process could then list or stop any thread's
  subscriptions. The asymmetry is deliberate and is the narrower of two
  choices. The realistic actor is an agent, and an agent is by construction a
  Lucidos-spawned subprocess carrying a token it cannot re-point, so the check
  covers exactly the caller whose isolation the tools promise. An untokened
  caller is the ordinary local API surface that every sibling
  `/threads/:id/...` route already trusts (`continue`, `answer-question`,
  `archive`, and the register route itself), and archiving a thread already
  stops all of its subscriptions, so refusing here would move a trust boundary
  without closing the capability. Widening it is a cross-cutting decision about
  the whole local API, not a property of these three routes: re-flag only with
  that decision made, or if an agent path is found that reaches them without a
  token.
  (`crates/lucidos-engine/src/api/threads/actions.rs`, ADR 0052.)

## Settled architecture questions

- **No shared turn-lifecycle orchestrator across the agent-session loop and
  the chat agentic loop** — ADR 0003. The seam already exists
  (`lifecycle.rs` pure decision functions + the typed terminal helpers).
- **External-repo coding-agent threads stay out of the change/dot/blocking
  machinery** — ADR 0001.

- **A rule cut from a tool schema is not a rule lost when the static system
  prompt states it** (2026-08-07, tool-schema budget trim). The trim's whole
  method is that where the prompt and a schema both carry a policy, the schema
  keeps a pointer or nothing and the prompt keeps the rule, so a reviewer
  reading only the diff sees deletions that look like dropped rules. The
  plugins "do NOT claim it succeeded" instruction was flagged this way and is
  in `system_prompt.rs`'s PLUGINS section verbatim. Before flagging a removed
  instruction, grep `system_prompt.rs`, `process_helpers.rs` and the
  `system-knowhow/*.md` body the schema now names. Re-flagging needs evidence
  that NO surface carries it, not that the schema no longer does.

- **`scaledDurationMs` is called at render time in three components and inside
  an effect in a fourth, and both are correct** (2026-08-09, animation-speed
  scale). A reviewer will read the split as an oversight in whichever direction
  they notice first: either "these three needlessly re-render when the slider
  moves" or "ContentPane's fuse never picks up a slider change". Neither is a
  bug, because the two shapes are asking for different things. A render-time
  read (`ThreadDrawer`, `ThreadView`, `AppUiInline`) feeds a `useLingeringFlag`
  whose linger is a *prop* held across renders, so it MUST subscribe or a
  mid-animation slider change would leave the old linger in place; the resulting
  re-render costs nothing at the rate a human drags a slider. An effect-body read
  (`ContentPane`'s nav-cover fuse) is evaluated fresh on each navigation, which
  is the only moment its value matters, so subscribing would buy a re-render for
  a value that is already re-read at every use. Re-flag only with evidence that a
  site's timer can now outlive a slider change without re-reading it.
  (`store/store.ts`, `components/layout/ContentPane.tsx`.)

- **A `SpawnRequest::Continue` spawn does NOT get a recovery system prompt, so
  `RESTART_NOT_REJECTION_RULE` does not reach it** (2026-08-10,
  answered-after-idle answer delivery). A reviewer looking at the
  `answered_after_idle` resume will reasonably assume the restart-not-rejection
  note from `docs/plans/2026-06-26-cc-restart-not-a-rejection.md` already covers
  it and flag the resume message's own framing as duplication, or flag "soft
  tension" between the two texts. It is neither: `resolve_run_worktree_context`
  selects `recovery_system_prompt` **only** when its `recovery_worktree`
  argument is `Some`, and the SpawnConsumer's Continue call passes
  `conflict_worktree` into that slot, which is `None` unless
  `resolve_continue_conflict_duty` found a merge duty. So an ordinary
  continuation runs on the plain `worktree_system_prompt` and carries no
  restart-not-rejection note at all, which is exactly why the framing has to
  ride in the message. Count the positional arguments before concluding
  otherwise: `run_direct_agent` takes sixteen, and `recovery_worktree` is the
  ninth. Re-flag only with evidence that the Continue path started passing a
  real recovery worktree or a `system_prompt_override`.
  (`engine/engine_impl/construction.rs`,
  `engine/agent_session/run_session/spawn_context.rs`.)

- **A fold key left in `collapsedExchanges` while `canCollapse` is false is
  intended, not a leak** (2026-08-10, turn collapse control). Reviewers land on
  this twice, from opposite ends: "the key is never pruned, so the fold
  re-applies and snaps a live turn shut with no user action", and "the reveals
  clear the key unconditionally, discarding a fold the user made deliberately".
  Both describe the same design and neither is a bug. `isCollapsed` is
  `canCollapse && has(key)`, so a step-only turn with the steps control off
  reads as unfolded while it is drawing nothing, and folds again when a row
  becomes drawable. That is the fold being honoured, and the two states are
  visually identical anyway (folded shows `⋯`, unfolded shows an empty body),
  which is what makes the transition invisible in practice. Pruning the key on
  the `canCollapse` false edge would silently discard the user's fold on every
  steps toggle; gating `expandExchange` on `canCollapse` would reintroduce the
  dead click it was added to remove, because the turn a reveal fires on is
  usually uncollapsible *because* the thing being revealed is hidden. Re-flag
  only with evidence that the two states became visually distinguishable, or
  that a key can be created for a turn the user never folded.
  (`components/chat/ChatExchange.tsx`, `store/store.ts`.)

- **The turn header's `align-items: flex-start` does NOT un-centre its icons,
  because every field is pinned to one row unit** (2026-08-10, turn header
  alignment; supersedes the entry that defended the centred version). This file
  carried the opposite prior for half a day: with `align-items: center`, a
  wrapped `.response-meta` / `.initiator-meta` is the tallest item on the row,
  so the chip and the turn controls centre against BOTH its lines and the
  executor's name lands half a line below the status. That was defended as a
  trade, on the grounds that flex-start would un-centre the icons against the
  label on every ordinary one-row header. The user reported the drop anyway,
  and the trade turned out to be avoidable: the headers now align at flex-start
  AND size every field to `--turn-header-line` (the chip via a min-height that
  adds back the padding its negative margin cancels, the turn controls
  directly, the cluster's first field directly), so the fields are equal-height
  and flex-start resolves to what centring gave. Measured in Chromium and
  WebKit: the one-row header is unchanged to the pixel, and the stacked status
  moves from 6.5px above the name's centre to exactly on it. So a reviewer
  proposing a return to `center`, or reading a lone field's min-height as
  redundant, is reading half the pair: neither half works alone, and dropping
  one field's unit drifts that field alone off the line. Pinned as a set in
  `styles/__tests__/turn-meta-stacking.test.ts`. Re-flag only with evidence
  that a field stopped resolving to the unit.
  (`styles/chat/response.css`, `styles/chat/input-messages.css`,
  `styles/global/base.css`.)

- **`was_attached: true` is unreachable, so an "attached wake loses its jump"
  finding is refuted by ADR 0049** (2026-08-10, the jump moved to the wake
  card). The reasoning is sound on its face and Codex produced it: an attached
  `EventWaitDelivered` is followed by the paired `await_event` `ToolResult`
  rather than by the `UserPromptInjected` that renders `EventDeliveryBody`, so
  moving the `Go to event` link off the arming row would leave such a delivery
  with no route to its matched event. What makes it moot is that the attached
  shape was retired on 2026-08-06 (ADR 0049, *Every event wait is detached*):
  **`was_attached` appears in no Rust file at all**, so the engine cannot emit
  one, and every delivery writes a `UserPromptInjected` re-entry anchor. The field
  survives only on the frontend wire type and its fixtures, for rows persisted
  before that date. Putting the link back on the arming card to serve those is
  the wrong trade twice over: the user removed it from there deliberately (it
  pointed at something that happened hours after the moment that card records),
  and a link appearing only on pre-2026-08-06 threads is inconsistent by
  construction. Re-flag only with evidence that the engine emits an attached
  delivery again, which would mean ADR 0049 was reversed.
  (`components/chat/chat-exchange-parts.tsx`,
  `store/thread-events/thread-event-types.ts`.)

- **`tryRestore` asking nothing about the container's position is the guard, not
  a missing one** (2026-08-10, scroll-memory restore window). The restore's
  retries write a saved offset over whatever `scrollTop` currently is, with no
  "has the reader moved" test beside them, and that reads as the yank the
  reader-owns-the-scroll rule exists to prevent. The test is there, one level up
  and asked correctly: the whole wait is retired by the reader's first GESTURE
  (`watchUserAction`, wheel / touchmove / pointerdown / keydown), so a retry can
  only ever run for a reader who has done nothing. A position test was tried
  first and removed after three reviewers independently refuted it: the app
  writes `scrollTop` all through that window without a navigation stamp (the
  browser clamping a shared container when shorter content swaps in,
  `restoreAfterReflow` across a pane resize, the render window compensating for
  prepended height, the iOS repaint nudge's ±1px five times per open), so a
  pixel delta reads our own writes as the reader and abandons the position for
  good. Re-flag only with evidence that an input event can reach `document`
  without the reader, or that a path arms the observers without arming the
  watch. (`hooks/useScrollMemory.ts`, `utils/userAction.ts`.)

- **The submit landing's FREEZING glide standing down for a same-owner tween is
  unreachable, not a missed supersede.** `glideToLiveEdge`
  (`components/chat/scrollState.ts`) leaves an in-flight glide of the same owner
  alone. A reviewer reasonably asks whether the hold's frozen last glide should
  outrank that, since a live-target tween is the chase the freeze exists to end.

  It cannot arise. A freeze is only ever asked for by `honourLanding`, and its
  three callers each exclude a running tween. `honourGrowth` returns on
  `_scrollAnimRaf !== null`. `animateScroll`'s `onDone` runs after
  `endScrollAnim`. And `followSubmit`'s direct call installs a landing whose
  resolver cannot answer on that same round: `awaitsNewTurn` needs a CHANGE from
  the snapshot it just took, and a card resolver's `drawnAtStart` equals its own
  reading.

  So the bypass is dead defensive code. A test for it cannot be written without
  driving the freeze from a path production has not got. Re-flag only if a
  freeze gains a caller that can run during a tween.
  (`components/chat/scrollState.ts`.)

- **The trash's optical correction reaching the queued-message trash too is the
  point of it, not spillover** (2026-08-11, Codex). `.icon-btn.row-icon
  .trash-icon` (`styles/global/host-components.css`) grows the glyph by the
  ratio of two measured fills, and `.queued-message-remove` carries `row-icon`,
  so the correction lands on the queued trash as well as the trigger group
  heading's. That reads as a targeted fix leaking onto an unrelated control,
  and the comment's "the pencil it sits beside" makes it read that way twice
  over, since nothing sits beside the queued one. The pencil is the REFERENCE
  for how much of its box a glyph in this icon set paints, not the reason: the
  trash under-fills wherever it renders, so the correction is right at every
  site taking this box, and the user asked for both places to grow in the same
  breath ("Same trash as queue? I guess we can make it taller both places?").
  Scoping it to `.trigger-group-delete` would put the two trashes back at
  different sizes in one nominal box, which is the inconsistency the shared
  class removed. Re-flag only with evidence that a `row-icon` site wants the
  UNCORRECTED glyph, which would be an argument for a second box, not a
  narrower selector. (`styles/global/host-components.css`,
  `styles/__tests__/trash-icon-optical-size.test.ts`.)

- **`onResize` writing the live edge twice for a box change that also grew the
  transcript is idempotent, not a redundant scroll** (2026-08-13). The box-change
  branch and the growth branch are two of `keepTheLiveEdge`'s three callers, so a
  resize that changes the box AND grows the content under an armed reader who was
  on the edge satisfies both, and the second call writes `liveEdgeTop(el)` again.
  That reads as a double write to a fresh reviewer, and it is, but it targets the
  same number: nothing between them changes `scrollHeight` or `clientHeight`, so
  the assignment is a no-op the browser fires no scroll event for. Collapsing
  them is not free, because the box-change caller is also the condition whose
  fallthrough runs `restoreAfterReflow`, and `honourGrowth` returns early on a
  pending landing, so folding the two would leave a box change with a landing in
  flight holding neither the correction nor the edge. That state is unreachable
  today (`armFollowOn` clears the landing and a submit made while armed installs
  none), and depending on it to place a write is exactly the coupling the early
  return should not acquire. Re-flag only if the callers stop sharing
  `keepTheLiveEdge`, or if something between them can change the container's
  geometry. (`components/chat/scrollState.ts`.)

- **`classify_build_failure` naming a `cargo clean` remedy for a build script
  that failed on a missing path is a SUGGESTION, not a repeatability verdict**
  (2026-08-14). The recognizer looks like it is deciding something load-bearing,
  and an earlier revision of it genuinely was: it set `repeatable` from that
  shape, which removed the toast's Retry button. Codex flagged it correctly:
  the same output comes from a build script whose input is genuinely absent,
  where a rebuild fixes it and a clean does not.

  The verdict now takes BOTH the recognized shape AND an observed repeat, which
  is why it looks redundant. Each half answers a false positive the other
  causes. The shape alone fires on a genuinely missing input. Repetition alone
  fires on two unrelated errors sharing one generic line (`error[E0308]:
  mismatched types`), which is a user midway through fixing things.

  Re-flag only if one half is dropped, or if a remedy string gains the power to
  remove an affordance on its own.
  (`crates/lucidos-engine/src/engine/engine_version.rs`,
  `crates/lucidos-app/src/store/actions/engine-update.ts`.)

- **`arm_question_resume_if_live` guards on `Canceled` alone while its three
  neighbours share `answer_resolves_without_resume`** (2026-08-15). It reads as
  an inconsistency somebody forgot to sweep, and it is deliberate.

  The shared predicate answers "does the engine owe this answer a follow-on
  turn?", and `Canceled` and `Superseded` both answer no. This function asks a
  different question: "is a live subprocess about to keep running?" A canceled
  thread is being torn down, so there is nothing to arm. A superseded one is
  not: it wakes, finishes the turn it was in, and reads the follow-up after
  that. Arming it is what stops those post-answer events being dropped as
  post-terminal stragglers.

  Re-flag only if `Superseded` stops implying a live continuing session, or if
  a third answer-less kind appears and needs sorting into the two groups.
  (`crates/lucidos-engine/src/engine/agent_question.rs`, ADR 0082.)

- **`lucidos-eval` passes its database URL as a `psql` argument** (2026-08-18).
  Looks like the `DATABASE_URL`-in-argv leak `.claude/rules/rust.md` bans, and
  it is a different situation.

  That rule is about the AGENT running `psql` through the Bash tool. The URL
  then lands in the persisted tool-call payload the steps UI renders.
  The eval harness is a binary spawning its own subprocess, so there is no
  transcript. The base is an operator-supplied local dev credential, and it is
  the only string that carries host, port and credentials for every arm.

  Re-flag if the harness runs `psql` through an agent tool, or if the base
  starts holding a credential that is not a local dev one.
  (`crates/lucidos-eval/src/workspace.rs`.)

- **A `CodingAgentToolResult` clears the event-wait park unconditionally, with
  no `tool_use_id` match** (2026-08-19). Reads as too broad, because an
  unrelated result would end the park while the wait's own call is pending. So
  a reviewer reaches for matching the wait's id against the result's.

  That match is not expressible. A wait a CODING agent arms comes through the
  CLI route, which mints its own `cli-<uuid>` for `EventWaitStarted.tool_use_id`
  (verified on a real thread). No `CodingAgentToolResult` ever carries that id,
  so an id-gated arm never fires and the park never ends. The chat agent's
  `await_event` does carry the model's id, and its result is a plain
  `ToolResult`, which this arm does not touch.

  What is left is honest. A coding-agent tool result means that subprocess is
  alive and answering, which is the opposite of a parked turn. An abandoned park
  produces none.

  Re-flag if the CLI route starts recording the agent's own tool id, or if the
  arm widens to the chat agent's `ToolResult`.
  (`crates/lucidos-app/src/store/thread-events/exchange-render.ts`.)

- **The 8-char short thread id keying a worktree directory is a system-wide
  identifier, not something the diff endpoint chose** (2026-08-21). Codex
  flagged it twice on the branch that taught `get_thread_cc_diff` to fall back
  to `deterministic_worktree_path`: two thread uuids sharing a prefix resolve
  to one directory, so one thread's diff could answer for the other.

  The observation is right and the altitude is wrong. `SHORT_THREAD_ID_LEN` is
  8 (`git_ops/worktree.rs`), and two consumers weigh more than this reader.
  `resume::resolve_worktree_path` hands both colliding threads the same spawn
  target, so they would share a worktree before anyone clicked Diff. And
  `worktree_cleanup_ops::parse_thread_short` recovers a full uuid by prefix
  lookup, then deletes what it resolved. A guard on the read side alone leaves
  both, and makes the Diff button stricter than the thing that built the tree.

  What the branch DID owe is the unknown-id case, which was new: an id naming
  no thread reaching a real one's worktree, unscoped. That is closed by
  `thread_owns_a_coding_agent_worktree`.

  Re-flag if the residual case is raised against `short_thread_id` itself, with
  the spawn and cleanup consumers in scope. A read-side-only guard is the fix
  this entry rejects.
  (`crates/lucidos-engine/src/api/repositories.rs`.)

- **The classifier pin reaches an arm through the harness's own `boot_engine`
  and not through the gateway's `spawn_engine`.** That is the harness topology,
  not a gap in the pin. A reviewer notices that `arm_engine_env` sets
  `LUCIDOS_FORCE_QUERY_CLASSIFICATION`, while
  `lucidos_gateway::stack::spawn_engine` builds its own set through
  `engine_env_overrides` and would spawn an unpinned engine.

  It would, and nothing about the pin changes that. The harness boots each arm
  itself, on the port the gateway registry already holds. The gateway therefore
  adopts that engine on the first proxy hit, rather than spawning a second one
  against the same database. A gateway spawn happens only if an arm engine has
  died, and such an engine loses the arm's whole configuration rather than just
  the pin: the harness's port, its resolved TLS pair, everything `boot_engine`
  passes. The run is already invalid at that point, and the void that pinning
  replaces is still in `analyse.rs` to catch the retrieval half of it.

  Re-flag if the gateway gains a path that spawns an arm engine on a healthy
  run, or if the harness stops booting its arms itself.
  (`crates/lucidos-eval/src/workspace.rs`, `crates/lucidos-gateway/src/stack.rs`.)

- **`DEVICE_ID_KEY` looks origin-global and is not.** A reviewer reads
  `localStorage.getItem('lucidos-device-id')` and notes that the gateway serves
  every workspace from one origin. The conclusion is that a migration guarded on
  that value runs once per browser, skipping every workspace but the first.
  Raised against the gateway device-identity adoption, where it would
  have stranded each later workspace's push subscription and preferences.

  It does not, because `installWorkspaceStorage` (`utils/workspaceStorage.ts`)
  overrides `getItem`/`setItem`/`removeItem` on the `localStorage` instance and
  prefixes every key with `ws:<slug>:`. `GLOBAL_KEYS` is the entire exemption
  list and holds two picker keys. That file says the device id in as many words:
  each workspace has its own device identity, deliberately. So the read is
  per workspace, and the migration runs once per workspace.

  Re-flag only if `lucidos-device-id` joins `GLOBAL_KEYS`, or if a caller reads
  it through a realm that bypasses the override. That file's header enumerates
  the three that do: the `index.html` FOUC IIFE, the engine-served
  `sdk-prefs.js`, and the SDK's `_storage.ts`. A fourth fails the build, at
  `no-raw-storage.test.ts`.
  (`crates/lucidos-app/src/utils/workspaceStorage.ts`,
  `crates/lucidos-app/src/utils/deviceIdHeader.ts`.)

- **A *model selection* is saved as two preference writes, and that is not a
  torn pair.** A reviewer reads `saveModelSelection`
  (`store/actions/preferences.ts`) and sees `savePreference(modelKey, …)` then
  `savePreference(reasoningKey, …)`. A network error between them leaves the
  new model stored beside the old effort, so the stated pairing is broken.

  The two writes really are sequential, and the pattern predates the helper:
  `setCurrentModel` has saved `chat_model` then `chat_reasoning_effort` the same
  way since the chat pair existed. What makes it survivable is that **neither
  end trusts the stored pair**. The picker clamps for display
  (`clampToOffered`), and `RoutingProvider::effort_for_model` clamps the request
  at the wire. A stale effort is therefore a value nothing acts on. That is why
  the pair is stored as two independent keys rather than one JSON value
  (ADR 0107). The failed write also surfaces: `savePreference` toasts.

  Re-flag if either clamp is removed, or if a caller starts reading the stored
  effort without one. Re-flag too if `PUT /api/v1/preferences` grows a
  multi-key write, which would make the pairing free.
  (`crates/lucidos-app/src/store/actions/preferences.ts`,
  `crates/lucidos-engine/src/llm/reasoning.rs`.)

- **The Devices list and an actor chip may call one device different things,
  and the list's name is the better one.** A reviewer reads
  `deviceDisplayName` (`components/settings/deviceList.ts`) beside the engine's
  `resolve_device_name` (`core/devices.rs`) and sees them diverge: for a device
  with no typed name but a gateway pairing, the list shows the pairing label
  and a chip shows `device-<first 8>`. Read as drift, that argues for dropping
  the label so both surfaces agree.

  Dropping it makes both surfaces worse. The pairing label is a name a person
  chose on the device itself. The short id is a fallback for having no name at
  all. The engine cannot reach the label: the paired-device store is a
  machine-global file the *workspace gateway* owns, and the engine has no
  handle on it. So agreement is only purchasable by showing the worse name
  twice. The bottom rung DOES match, so an unnamed and unpaired device reads
  the same everywhere.

  Re-flag if the engine gains a way to read the pairing label, which would make
  agreement free. Re-flag too if an engine row starts adopting that label as
  its name at registration.
  (`crates/lucidos-app/src/components/settings/deviceList.ts`,
  `crates/lucidos-engine/src/core/devices.rs`.)

- **The three underline affordances are not one class waiting to be extracted,
  and the audience split is why.** `.accent-link`
  (`styles/global/shared-components.css`), `.event-name-link`
  (`styles/chat/event-rows.css`) and `.event-wait-subscription-filter`
  (`styles/chat/waiting-indicator.css`) each carry a button reset plus the same
  40% accent underline, ramping to full on hover. Read as copy-paste, that
  argues for one shared atom the three compose with.

  The first of the three cannot compose with anything. `shared-components.css`
  is `include_str!`d by the engine and served to every app iframe. So a base
  class living there ships to apps, and one living anywhere else is unreachable
  from it. Moving `.accent-link` out is worse still, since it is
  part of the documented app-facing class contract. That leaves a two-copy
  extraction across two chat files, where the copies already differ in what
  they do with colour: `.event-name-link` keeps its chip background and darkens
  it on hover, and the other two only tint text.

  Re-flag if a fourth copy appears. Re-flag too if the two chat rules converge
  on identical colour behaviour, which would make a chat-local base class carry
  its weight.

- **Two glossary entries still say "panel-less" where the term is now
  *Escape-only registrant*, and that is deliberate.** `docs/glossary.md`'s
  § Overlay stack and § Thread filter panel each use the adjective in a
  descriptive clause. A reviewer applying `.claude/rules/glossary.md`'s
  one-name-root rule reads that as a half-finished rename.

  Both clauses sit inside single-line paragraphs of ten and eight sentences,
  several of them well over the word limit. `scripts/check-prose.sh` scores an
  edited line as added. So retouching either one to swap two words makes every
  pre-existing breach in that paragraph a failure to fix. `.claude/rules/prose.md`
  forbids exactly that: "Never start a repo-wide sweep on your own." The term
  itself IS defined once, in the paragraph appended to § Overlay stack. Neither
  older clause claims to be a definition or an exhaustive list.

  Re-flag when either paragraph is being rewritten for its own sake, which is
  when the swap becomes free. Re-flag too if a THIRD spelling appears, since the
  argument here is only that the two survivors are ordinary adjectives.
