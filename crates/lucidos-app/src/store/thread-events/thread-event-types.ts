import type { EventChannel, SessionEndReason } from '../../generated/thread-lifecycle';
import type { CodingAgent } from '../../api/types';
import type { ContextSection } from '../types';

export type ThreadInitiator = 'user' | 'system';

/** Mirrors the Rust `ActorMode` enum (lowercase strings). */
export type ActorMode = 'human' | 'agent' | 'engine';

/** Mirrors the Rust `EngineReason` enum (serde tag = "kind", snake_case).
 *  `session_recovered` is the legacy name for `continuation_started` — kept
 *  on the union so legacy DB rows still typecheck during the migration window
 *  (the Rust side has the matching `#[serde(alias = "session_recovered")]`,
 *  and the migration rewrites historical rows to the new name). */
export type EngineReason =
  | { kind: 'continuation_started' }
  | { kind: 'session_recovered' }
  | { kind: 'orphan_recovery' }
  | { kind: 'scheduler'; trigger_id: string; trigger_name?: string }
  | { kind: 'harden_retrigger' }
  | { kind: 'stale_session' }
  | { kind: 'merge_conflict' }
  | { kind: 'missing_hardening' }
  | {
      kind: 'plugin_auto_update';
      plugin_id: string;
      marketplace_id: string;
      marketplace_name: string;
    };

/** Direction of a `thread_link` origin: which end of the parent⇄child
 *  relationship the linked thread sits on relative to the receiving thread. */
export type ThreadDirection = 'parent' | 'child';

/** Mirrors the Rust `MessageOrigin` enum (serde tag = "kind", snake_case).
 *  `api.source_thread_id` populated when the request came from a
 *  Lucidos-spawned subprocess (CC, `run_bash`, `run_python`, scheduled
 *  script, `lucidos` CLI). The engine reads it out of the thread-bound
 *  origin token in `x-lucidos-agent-origin-token`, whose own prefix names
 *  the spawning thread, so the value is authenticated rather than claimed.
 *  When set, `mode === 'agent'` (subprocesses are never human) and the
 *  popover renders a deep-link back to the spawning thread. */
export type MessageOrigin =
  | { kind: 'device'; device_id: string; label: string }
  | { kind: 'api'; user_agent?: string; mode?: ActorMode; source_thread_id?: string }
  | {
      kind: 'workspace';
      workspace: string;
      thread_id?: string;
      event_id?: string;
      user_agent?: string;
      mode?: ActorMode;
    }
  | {
      kind: 'thread_link';
      thread_id: string;
      title?: string;
      spawning_event_id?: string;
      mode?: ActorMode;
      direction?: ThreadDirection;
    }
  | { kind: 'engine'; reason: EngineReason }
  /** The host system killed the underlying process (engine shutdown, OS signal,
   *  crash, safety-net catch). Distinct from `engine` which represents
   *  engine-deliberate actions (hardening retrigger, scheduler, merge conflict). */
  | { kind: 'system' };

/** The five actor labels below are the WHOLE of what this module says about the
 *  actor chip. Every chip's glyph is a component and every one of them resolves
 *  in the view layer, in `actorInitiator` / `describeExecutor`
 *  (`ChatExchange.tsx`), so this store module stays free of UI components. Two
 *  of the five used to break that rule as bare emoji string constants
 *  (`SYSTEM_ICON`, `API_CALLER_ICON`), which read as store data only because a
 *  string is not a component; both are gone and their reasoning now lives on the
 *  icons themselves in `components/shared/icons.tsx`. Do not add a `*_ICON` here.
 *
 *  Display label for engine-deliberate work (hardening, merging, scheduler). Its
 *  chip icon is the Lucidos brand mark, the SAME glyph as the *Lucidos Agent*,
 *  so the label is what distinguishes the two Lucidos actors at a glance. */
export const ENGINE_LABEL = 'Lucidos Engine';

/** Display label for process killed by the host system (engine shutdown,
 *  safety-net catch, OS signal). Distinct from `ENGINE_LABEL`: the engine
 *  acts deliberately; the system just kills processes. */
export const SYSTEM_LABEL = 'System';

/** Display label for work kicked off by a Lucidos LLM agent in another thread
 *  (parent_thread origin) — distinct from the engine, which only owns events
 *  it literally raises on its own (recovery, hardening, scheduler, …). */
export const LUCIDOS_AGENT_LABEL = 'Lucidos Agent';

/** Display label for an external HTTP caller that did NOT self-identify as a
 *  known actor (no device id, no agent-origin token, no cross-workspace
 *  caller). The popover surfaces the User-Agent for forensics; the chip just
 *  says "API caller" so the user never sees an anonymous mutation rendered as
 *  "You". */
export const API_CALLER_LABEL = 'API caller';

/** Derive the ActorMode from a MessageOrigin. Mirrors the Rust
 *  `MessageOrigin::mode()` impl: device is intrinsic Human, engine and system
 *  are intrinsic Engine, the others read from the carried `mode` field
 *  (defaulting to the same defaults the backend uses for old DB rows). */
export function originMode(origin: MessageOrigin | undefined): ActorMode {
  if (!origin) return 'engine'; // unknown origin → engine acted on its own
  switch (origin.kind) {
    case 'device':      return 'human';
    case 'api':         return origin.mode ?? 'human';
    case 'workspace':   return origin.mode ?? 'human';
    case 'thread_link': return origin.mode ?? 'agent';
    case 'engine':      return 'engine';
    case 'system':      return 'engine';
  }
}

/** Mirrors Rust's `CancelCause` (snake_case). User-driven termination of a
 *  *real* in-flight response — `EventMeta.actor` identifies the user, this
 *  enum identifies what they did. `superseded_by_followup` is the Codex
 *  mid-turn follow-up redirect: the user steered (didn't Stop), so it renders
 *  neutrally — like the chat/CC follow-up — instead of "Canceled ✕" (see
 *  `exchangeStatus` and `exchange-grouping`). `unknown` covers legacy DB rows
 *  persisted before the field existed and any retired cause string (e.g.
 *  `stale_settle`, which moved to `AbortCause`). */
export type CancelCause = 'user_stop' | 'user_action' | 'superseded_by_followup' | 'unknown';

/** Mirrors Rust's `AbortCause` (snake_case). System-driven cleanup — the
 *  engine or OS terminated the process, or the engine settled a projection
 *  whose live process was already gone (`stale_settle`). `unknown` only
 *  appears on legacy DB rows persisted before the field existed. */
export type AbortCause =
  | 'engine_shutdown'
  | 'safety_net'
  | 'recovery_after_restart'
  | 'process_killed'
  | 'stale_settle'
  | 'session_dropped'
  | 'unknown';

/** True when the abort is the engine's OWN teardown, whoever asked for it.
 *
 *  One of the two halves of the switch fingerprint below, and the one that says
 *  the PROCESS is going down. It is a different claim from the other half (a
 *  device actor, i.e. a user asked), and it licenses different things:
 *
 *  - **This one** says nothing can be running under the boundary, because there
 *    is no engine left to run it. That is what the transcript's "is this turn
 *    live" readings need (`abortTookEngineDown` in `exchange-render.ts`).
 *  - **Both together** say the engine PROMISED to bring the turn back, which is
 *    what the `paused` verdict, the "Paused by restart" wording, the withheld
 *    Continue button and the auto-resume need.
 *
 *  Separated on 2026-08-13, because both live-ness readings were keyed on the
 *  full fingerprint and so only ever applied to an attributed switch. An
 *  UNATTRIBUTED shutdown (a terminal `stop.sh`, an external SIGUSR1, ctrl-c:
 *  each leaves `LucidosEngine::teardown_actor` `None`) produces the identical
 *  boundary and the identical drain, and kept the shimmering "Working" header
 *  plus a derived live "Thinking" row over a subprocess that no longer existed.
 *  Real thread b146c294: the abort at 12:29:27.801, its drain `"\n\n"` 12ms
 *  later, then 24 seconds of "Thinking" while the engine was down.
 *
 *  Deliberately NOT "is this an abort at all". A `safety_net` abort fires on a
 *  turn the watchdog only THOUGHT was stuck, the loop keeps going, and real work
 *  lands under that boundary (real thread ebc787a4). Only a shutdown takes the
 *  engine with it. */
export function isEngineDownAbort(cause: AbortCause | undefined): boolean {
  return cause === 'engine_shutdown';
}

/** True for the teardown boundary of a user-initiated *Switch to new version*:
 *  an `engine_shutdown` abort stamped with the device that clicked Switch.
 *
 *  The single frontend definition of the fingerprint, mirroring the backend's
 *  `SWITCH_TEARDOWN_ABORT_SQL` (`agent_recovery/recovery.rs`) and its in-Rust
 *  twin `AbortCause::promises_auto_resume` (`thread_events/cause.rs`). Matching
 *  means the engine PROMISED to resume this turn, and all three consequences
 *  key on this one predicate so they cannot disagree: the thread reads `paused`
 *  (backend), the transcript says "Paused by restart", and the Continue button
 *  is withheld (see `abortPromisesAutoResume`).
 *
 *  **Both halves are load-bearing.** A device actor alone is not the
 *  fingerprint: `stale_settle` deliberately carries the actor of whichever
 *  button exposed a stuck row (Stop / Apply / Discard / Archive / Interrupt).
 *  Nor is `engine_shutdown` alone: the shutdown fallback for a thread that
 *  started after the restart pre-emit carries a system actor, and no resume gate
 *  picks that up. Built ON the cause half above rather than re-spelling it, so
 *  "the engine went down" cannot come to mean two things; what this adds is the
 *  actor. */
export function isSwitchTeardownAbort(
  actor: MessageOrigin | undefined,
  cause: AbortCause | undefined,
): boolean {
  return isEngineDownAbort(cause) && actor?.kind === 'device';
}

/** Summary text for a `ResponseAborted` event. `stale_settle` (engine cleanup
 *  of a stuck projection on a user button click) reads "Settled stuck
 *  response" — distinct from a real abort because no live response existed.
 *  The user's own switch reads "Paused by restart", matching the `paused` thread
 *  status the same abort leaves behind (the turn is parked, not lost, and
 *  resumes on its own). Anything else is an interruption nobody promised to
 *  undo, which reads "Response interrupted" over a `failed` thread. */
export function responseAbortedSummary(
  actor: MessageOrigin | undefined,
  cause: AbortCause | undefined,
): string {
  if (cause === 'stale_settle') return 'Settled stuck response';
  return isSwitchTeardownAbort(actor, cause) ? 'Paused by restart' : 'Response interrupted';
}

/** True when this event is **the user pressing Stop waiting** on a live *thread
 *  subscription*, as opposed to any of the other ways one ends.
 *
 *  The single definition of that fingerprint, because three surfaces key on it
 *  and must not disagree: grouping opens an exchange for it, the response
 *  projection then leaves the arming row alone, and the initiator panel renders
 *  it as the user action it was.
 *
 *  Only `user_stop` qualifies. An `agent_stand_down` is the agent retiring a
 *  watch mid-turn, which belongs as a step inside that turn, and the two
 *  thread-lifecycle causes ride an archive or a discard that already reads as
 *  its own thing.
 *
 *  Takes the two fields it reads rather than a `ThreadEvent`, so the grouping
 *  fold can hand it a `StoredEvent` and a test can hand it a literal. Every
 *  `cause` on the union is a string enum, so the widened type accepts all of
 *  them. */
export function isUserStoppedWait(event: { type: string; cause?: string }): boolean {
  return event.type === 'EventWaitCanceled' && event.cause === 'user_stop';
}

/** The wait's `reason` as a bare subject, for a label that already said "wait".
 *
 *  Both transcript labels prefix the model's own words with a template carrying
 *  the verb. A reason opening "waiting for the e2e lock" would otherwise render
 *  as `Stopped waiting: waiting for the e2e lock`. No template avoids that:
 *  every label for this concept contains a waiting word.
 *
 *  At the label rather than at the stored reason, because the text is the
 *  model's and belongs on disk as written. The *waiting indicator* calls it in
 *  the one place it supplies a verb, its aria-label. Its tooltip says the
 *  reason alone, so that one takes the text as written.
 *
 *  Three judgments sit behind the two lines below, and each is a decision
 *  rather than a gap: `to` is not one of the prepositions, only a LEADING
 *  phrase goes, and a reason that is nothing else comes back whole. See
 *  `docs/plans/2026-08-14-a-wait-label-does-not-say-waiting-twice.md`.
 *
 *  `core::tool_label` carries the engine-side twin, for the pending step. */
export function awaitedSubject(reason: string): string {
  const stripped = reason.replace(/^\s*wait(?:ing)?\s+(?:for|on|until)\s+/i, '');
  return stripped.trim() ? stripped : reason;
}

/** Header label for the turn a user's **Stop waiting** opens.
 *
 *  Says what was stopped, in the model's own words, because that is the only
 *  thing on screen that names the subscription once the clock indicator has
 *  dropped it. A pre-2026-08-07 `EventWaitCanceled` carries no reason, and the
 *  line then says the one thing it knows rather than trailing an empty colon.
 *
 *  Deliberately the same wording as the transcript's stop row, which is what a
 *  NON-user stop renders: one phrasing for one concept, whichever surface it
 *  lands on. Both therefore inherit `awaitedSubject`. */
export function eventWaitStoppedSummary(reason: string | undefined): string {
  return reason ? `Stopped waiting: ${awaitedSubject(reason)}` : 'Stopped waiting for an event';
}

/** Header label / preview text for a `ResponseCanceled` turn — always a
 *  user-driven stop on a real in-flight response, so no cause-dependent
 *  branching is needed. Rendered as the turn's header (no actor chip); the
 *  cancel cause is surfaced in the Initiator info popover instead. */
export const RESPONSE_CANCELED_SUMMARY = 'Response canceled';

/** ContinuationStarted.reason emitted when the engine auto-resumes a coding
 *  agent after its subprocess died WITHOUT an engine restart — a hung-API
 *  watchdog fire OR a stray signal-kill (e.g. another workspace's `cargo check`
 *  broad-kill landing on this CC subprocess). Distinct from a user clicking
 *  "continue" after a real restart, which DOES warrant the restart wording. */
export const CONTINUATION_AUTO_RECOVERY_REASON = 'auto_recovery_after_hang';

/** ContinuationStarted.reason emitted when the user clicked Continue on an
 *  interrupted response. Mirrors Rust's `USER_CLICKED_CONTINUE_REASON`. This
 *  path also stamps the clicking device on the actor, so the popover shows a
 *  Device row alongside the explainer. */
export const CONTINUATION_USER_CLICKED_REASON = 'user_clicked_continue';

/** ContinuationStarted.reason emitted when the engine auto-resumes a
 *  coding-agent thread that was in flight during a user-initiated *Switch to
 *  new version*. Mirrors Rust's `AUTO_RESUME_AFTER_SWITCH_REASON`, which is
 *  stamped on the coding-agent resume path alone (`engine_version.rs`): a chat
 *  or trigger thread auto-resumed by the same Switch records no reason at all
 *  (`emit_resume_anchor`) and falls back to the generic engine explanation. The
 *  device that pressed Switch is recorded on the teardown `ResponseAborted`,
 *  not here, so the resume itself carries no actor. */
export const CONTINUATION_AUTO_RESUME_AFTER_SWITCH_REASON = 'auto_resume_after_switch';

/** ContinuationStarted.reason emitted when the engine resumes a coding-agent
 *  turn the backend ended on a TRANSIENT upstream failure it reported itself
 *  (its own `API Error: …`, e.g. a connection closed mid-response). Mirrors
 *  Rust's `AUTO_RESUME_AFTER_API_ERROR_REASON`. Nothing restarted here either:
 *  the previous turn's `ResponseFailed` is in the timeline right above, and this
 *  is the engine picking the same work back up. */
export const CONTINUATION_AUTO_RESUME_AFTER_API_ERROR_REASON = 'auto_resume_after_api_error';

/** `CodingAgentIdled.reason` stamped by crash recovery on the synthetic idle it
 *  emits directly beneath its own `ResponseAborted` boundary. Mirrors Rust's
 *  `ENGINE_RESTART_INTERRUPT_REASON` (`agent_recovery/helpers.rs`). It is the
 *  engine SAYING a mid-turn session was interrupted, not a turn reporting that
 *  it finished, and `continuableAbortIndex` has to tell those apart to know
 *  whether the boundary still wants a Continue button. */
export const IDLE_ENGINE_RESTART_INTERRUPT_REASON = 'engine_restart_interrupt';

/** Header label / preview text for a `ContinuationStarted` turn. The reason
 *  takes precedence: an `auto_recovery_after_hang` or
 *  `auto_resume_after_api_error` resume is a LOCAL interruption (a hang, a stray
 *  signal-kill, an upstream drop), never an engine restart, so it must not claim
 *  "Resumed after engine restart" (which once made a user think restarting an
 *  unrelated workspace had restarted theirs). A human actor means the user
 *  clicked Continue; anything else on a restart-recovery continuation is the
 *  engine resuming after a real restart.
 *
 *  The two local interruptions get their OWN wording rather than a shared
 *  "Resumed after an interruption". They can happen minutes apart on one thread
 *  (an upstream drop, then the hang watchdog on the session that replaced it),
 *  and two identical rows told the user neither what had happened nor that the
 *  causes differed. Each label mirrors its `describeContinuationReason`
 *  explainer, and neither claims a restart. */
export function continuationStartedSummary(
  reason: string | undefined,
  actor: MessageOrigin | undefined,
): string {
  if (reason === CONTINUATION_AUTO_RECOVERY_REASON) {
    return 'Resumed after the session stopped responding';
  }
  if (reason === CONTINUATION_AUTO_RESUME_AFTER_API_ERROR_REASON) {
    return 'Resumed after the model connection dropped';
  }
  return originMode(actor) === 'human' ? 'Continued the response' : 'Resumed after engine restart';
}

// Persisted thread events — stored in DB, appear in snapshots.
// Optional fields (`?`) allow older DB rows (before the field was added) to deserialize safely.
/** Mirrors the Rust `ChildCompletionStatus` (serde rename_all = "snake_case").
 *  Drives the status badge on the child-completion row. */
export type ChildCompletionStatus = 'success' | 'failure' | 'no_changes' | 'canceled';

/** Mirrors the Rust `AllowScope` (serde rename_all = "snake_case"). Carried on
 *  `CodingAgentPermissionResolved` so the answered card can render the same
 *  button layout as the prompt with a check on the chosen scope and
 *  strike-through on the rest. `undefined` covers Allow-once, Deny, and
 *  recovery-emitted orphan resolutions (no scope was picked). */
export type PersistScope = 'narrow' | 'broad' | 'session';

/** Mirrors the Rust `EventSubscription` (`core::event_subscription`). One entry
 *  in a subscriber's `on:` list: an event name plus an optional condition. A
 *  condition key is a field path: a dot reads one level down. Operators:
 *  `$eq/$ne/$lt/$lte/$gt/$gte/$in/$nin/$regex`, plus `$or` in key position
 *  taking a list of conditions. A trigger's `on:` and a thread's event wait
 *  are the same shape and run the same matcher, so this one type serves
 *  both. */
export interface EventSubscription {
  event_type: string;
  condition?: unknown;
}

/** Mirrors the Rust `EventWaitCancelCause` (serde rename_all = "snake_case"):
 *  how a *thread subscription* was stopped short of its own resolution.
 *
 *  Two arms are not what they look like. `thread_canceled` is RETIRED and only
 *  ever read: a thread-level Stop used to stop every subscription on the thread
 *  and no longer does, but rows written before 2026-08-07 carry it. And a
 *  timeout is not here at all, since that is `EventWaitExpired`.
 *
 *  Note what is absent: an ordinary message into a subscribed thread runs a
 *  normal turn and leaves every subscription live, so a passing question cannot
 *  silently throw away a long watch. */
export type EventWaitCancelCause =
  | 'user_stop'
  | 'agent_stand_down'
  | 'thread_archived'
  | 'thread_discarded'
  | 'thread_canceled'
  | 'unknown';

export type ThreadEvent =
  | { type: 'MessageReceived'; text: string; channel?: EventChannel; user_image_hashes?: string[]; device_id?: string; device?: string; image_description?: string; mode?: ActorMode; model?: string; reasoning_effort?: string; parent_thread_id?: string; spawning_event_id?: string; origin?: MessageOrigin }
  | { type: 'QueuedMessageRemoved'; removed_message_id: string; actor?: MessageOrigin; channel?: EventChannel }
  | { type: 'TextStreamed'; text: string }
  | { type: 'ThoughtStreamed'; text: string; context_tokens?: number; context_messages?: number; trimmed?: boolean }
  // ContextTokensMeasured / ContextAssembled are legacy event types — old DB
  // rows still surface them; new emissions use ContextCaptured below. Kept on
  // the union so the projection's switch can branch on them without `as`.
  | { type: 'ContextTokensMeasured'; input_tokens: number }
  | { type: 'ContextAssembled'; sections: ContextSection[]; tools: string[]; model: string; total_chars: number }
  | {
      type: 'ContextCaptured';
      producer: 'main_llm' | 'claude_code' | 'codex' | 'auxiliary';
      model: string;
      context_window: number;
      /** Absent on snapshot rows (server strips for size — see
       *  `strip_context_capture_sections` in `api/threads.rs`). Live SSE
       *  emissions carry the full array; lazy-fetch covers the snapshot
       *  case via `GET /events/:event_id/context`. */
      sections?: ContextSection[];
      tools?: string[];
      estimated_total_tokens: number;
      usage?: { input_tokens: number; output_tokens: number; cache_read_tokens: number; cache_creation_tokens: number };
      trimmed?: boolean;
      /** Which trim passes cut something, ascending. `trimmed` says a round
       *  lost content and this says where from. Pass 5 is the only one that
       *  removes a message instead of leaving an addressed stub. Absent on
       *  every row written before the field existed. */
      trim_passes?: number[];
      /** Stamped by the snapshot endpoint when `sections` + `tools` were dropped. */
      sections_stripped?: boolean;
      /** Which model call this records. Absent means `turn`, an agent's own
       *  round trip, which is the only kind the transcript renders. Anything
       *  else is an *auxiliary model call* the engine made for itself, and
       *  `isAuxiliaryCapture` keeps it out of the exchange fold. */
      purpose?: 'turn' | 'title' | 'image_describe' | 'memory' | 'conversation_summary' | 'image_gen';
      /** Rebuilt after the fact from other events, so its numbers are
       *  estimates and it carries no `usage`. */
      reconstructed?: boolean;
    }
  // The engine's automatic pre-turn recall: it vector-searched memory with
  // classifier-derived `queries` and injected the hits before the model ran.
  // NOT the agent's own lookup, which arrives as a ToolCalled for the `memory`
  // tool's `search` action. MemorySearched is the pre-rename name; old DB rows
  // still surface it, and the snapshot endpoint serves the raw `event_type`
  // column, so the serde alias on the Rust variant never reaches us.
  | { type: 'MemoryRecalled'; results?: number; queries?: string[] }
  | { type: 'MemorySearched'; results?: number; queries?: string[] }
  | { type: 'ToolCalled'; name: string; args: unknown; description?: string }
  | {
      type: 'ToolResult';
      name: string;
      /** Absent on snapshot rows (server strips for size — see
       *  `strip_tool_result_content` in `api/threads.rs`). Live SSE emissions
       *  carry the full text; lazy-fetch covers the snapshot case via
       *  `GET /events/:event_id/tool-result`. */
      result?: string;
      images?: string[];
      tool_called_event_id?: string;
      /** Stamped by the snapshot endpoint when `result` was dropped. */
      result_stripped?: boolean;
    }
  | { type: 'TodoListWritten'; items: TodoItem[]; notes?: string }
  // The agent's picture of the job under the self-curated context mode,
  // written as ordinary text in its own reply. Rendered as a folded step: the
  // user can read what the agent decided without it landing in the chat.
  | { type: 'WorkingUnderstandingWritten'; document: string }
  | { type: 'ResponseGenerated'; text?: string; images?: string[]; model?: string; reasoning_effort?: string; request_event_id?: string; channel?: EventChannel }
  | { type: 'ResponseCanceled'; text?: string; images?: string[]; model?: string; reasoning_effort?: string; actor?: MessageOrigin; channel?: EventChannel; cause?: CancelCause }
  | { type: 'ResponseAborted'; text?: string; images?: string[]; model?: string; reasoning_effort?: string; request_event_id?: string; actor?: MessageOrigin; channel?: EventChannel; cause?: AbortCause }
  | { type: 'ResponseFailed'; error: string; request_event_id?: string; channel?: EventChannel }
  | { type: 'SessionStarted'; session_id: string; branch?: string; repo_id?: string; coding_agent?: CodingAgent }
  // `reason` mirrors the originating ContinuationRequested.reason. It lets the
  // timeline label the resume honestly: 'user_clicked_continue' is a genuine
  // resume after an engine restart, but 'auto_recovery_after_hang' fires for a
  // hung subprocess OR a stray signal-kill (e.g. a cross-workspace `cargo check`
  // broad-kill) where nothing restarted. Absent on legacy rows + the chat rerun.
  | { type: 'ContinuationStarted'; branch?: string; origin?: MessageOrigin; actor?: MessageOrigin; reason?: string }
  // SessionEnded.reason is loosely typed to tolerate legacy DB rows whose
  // payloads carry removed values like 'completed' / 'changes_proposed' /
  // 'auto_ended' / 'user_ended' / 'stale_resume' / 'discarded'. The current
  // (Phase 4) terminal-only reasons are 'shutdown' / 'panic' / 'closed' plus
  // the engine's 'legacy_non_terminal' catch-all; everything else is a
  // historical row and should be treated as a harmless terminal end.
  | { type: 'SessionEnded'; reason?: SessionEndReason | string }
  | { type: 'CodingAgentTextStreamed'; text: string; coding_agent?: CodingAgent }
  | { type: 'CodingAgentThoughtStreamed'; text: string; coding_agent?: CodingAgent }
  | { type: 'CodingAgentToolCalled'; name: string; args: unknown; description?: string; tool_use_id?: string; coding_agent?: CodingAgent }
  | { type: 'CodingAgentToolResult'; name: string; result: string; tool_use_id?: string; coding_agent?: CodingAgent }
  | { type: 'CodingAgentUserMessageSent'; text: string; coding_agent?: CodingAgent }
  | { type: 'CodingAgentPromptSent'; text: string; origin?: MessageOrigin; coding_agent?: CodingAgent }
  | { type: 'MissingHardeningDetected'; origin?: MessageOrigin }
  // `reason` is absent on an ordinary idle (the agent simply finished its turn)
  // and carries `IDLE_ENGINE_RESTART_INTERRUPT_REASON` on the synthetic idle
  // crash recovery emits under its own abort boundary.
  | { type: 'CodingAgentIdled'; has_changes?: boolean; requires_restart?: boolean; is_external_repo?: boolean; cc_session_id?: string; coding_agent?: CodingAgent; reason?: string }
  // Continuation requested — emitted when an interrupted CC turn (engine restart
  // mid-turn, watchdog, missing-hardening sweep) needs to resume without a new
  // user message. The spawn dispatcher picks it up and re-spawns via --resume.
  // Past name `ContinueSignal` (serde alias on the Rust side). `reason` is a
  // short informational tag (e.g. "engine_restart_interrupt").
  | { type: 'ContinuationRequested'; reason?: string }
  | { type: 'ThreadTitleGenerated'; title: string }
  | { type: 'ThreadTitleRenamed'; title: string; actor?: MessageOrigin }
  | { type: 'ThreadSaved'; actor?: MessageOrigin }
  | { type: 'ThreadUnsaved'; actor?: MessageOrigin }
  | { type: 'ThreadArchived'; actor?: MessageOrigin }
  | { type: 'ThreadStarted'; mode: string; actor?: MessageOrigin }
  | { type: 'ThreadDiscarded'; actor?: MessageOrigin; discarded_at?: string }
  // A user attached an image to a compose draft (POST /threads/:id/blobs).
  // `hash` (sha256) is the sole identity downstream consumers use; `mime` /
  // `byte_size` are convenience fields for rendering the upload entry.
  | { type: 'ImageUploaded'; hash: string; mime: string; byte_size: number; actor?: MessageOrigin }
  /** A trigger thread's STARTER event (it has no `MessageReceived`), so
   *  `model` / `reasoning_effort` are what the per-thread model memory reads
   *  for such a thread. They record what the fire actually ran on: the
   *  trigger's own pin when it has one, otherwise the account chat default. */
  | { type: 'TriggerStarted'; trigger_id: string; trigger_name?: string; prompt?: string; invocation?: TriggerInvocation; origin?: MessageOrigin; model?: string; reasoning_effort?: string }
  | { type: 'TriggerCompleted'; trigger_id: string; trigger_name?: string; result_summary?: string }
  | { type: 'ChangeProposed'; change_id?: string; description?: string; files?: string[]; requires_restart?: boolean; path?: string; diff?: string; origin?: MessageOrigin; commit_sha?: string; incomplete?: boolean }
  | { type: 'ChangeApplied'; change_id?: string; requires_restart?: boolean; client_update?: boolean; commits?: string[]; thread_title?: string; actor?: MessageOrigin; path?: string }
  | { type: 'ChangeDiscarded'; change_id?: string; actor?: MessageOrigin; path?: string }
  | { type: 'ChangeReverted'; change_id?: string; actor?: MessageOrigin; path?: string }
  | { type: 'ChangeApplyFailed'; change_id?: string; error?: string; actor?: MessageOrigin }
  // A change's working tree was hardened (`/harden` marker stamped on HEAD).
  // Passive change-lifecycle bookkeeping — projection tracks only the latest
  // event per change_id.
  | { type: 'ChangeHardened'; change_id?: string; actor?: MessageOrigin }
  | { type: 'MergeConflictDetected'; change_id?: string; files?: string[]; origin?: MessageOrigin }
  // Merge-resolution worktree lifecycle. Started sets the change's
  // merge_worktree_path / merge_temp_branch until Cleared tears it down.
  // Both survive restart so startup cleanup can find dangling worktrees.
  | { type: 'MergeResolutionStarted'; change_id?: string; worktree_path?: string; temp_branch?: string }
  | { type: 'MergeResolutionCleared'; change_id?: string }
  /** `delivered_event_id` is set ONLY on an event-delivery anchor: the id of
   *  the `EventWaitDelivered` this injection carries. `text` spells
   *  the matched event out as pretty-printed JSON because it is the prompt the
   *  model reads, so rendering it verbatim gives the user a screen of raw JSON.
   *  Follow the id to that event instead and render its `event_type` /
   *  `payload` as a named event with the payload folded away. Absent on every
   *  other injection, on an expiry re-entry, and on rows that pre-date the
   *  field, where the prose IS the content. */
  | { type: 'UserPromptInjected'; text: string; mode?: ActorMode; origin?: MessageOrigin; injected_message_id?: string; delivered_event_id?: string }
  | { type: 'CredentialRequested'; provider: string }
  | { type: 'McpConsentRequested'; tool: string; args: unknown }
  | { type: 'CodingAgentSettingsChanged'; model?: string; reasoning_effort?: string; permission_mode?: string; cc_session_id?: string; coding_agent?: CodingAgent }
  | { type: 'UserQuestionAsked'; tool_use_id: string; cc_session_id: string; question: string; options?: QuestionOption[]; multi_select?: boolean }
  | { type: 'UserQuestionAnswered'; tool_use_id: string; answer: AnswerKind; actor?: MessageOrigin }
  | { type: 'CodingAgentPermissionRequest'; request_id: string; tool_use_id: string; tool_name: string; input: Record<string, unknown>; summary: string }
  | { type: 'CodingAgentPermissionResolved'; request_id: string; allowed: boolean; reason?: string; persist_scope?: PersistScope; actor?: MessageOrigin }
  // Command guard (ADR 0002) — the chat mirror of the CodingAgentPermission* pair.
  // Carries the inspected `command` text instead of CC's structured tool `input`.
  | { type: 'CommandPermissionRequested'; request_id: string; tool_use_id: string; tool_name: string; command: string; summary: string }
  | { type: 'CommandPermissionResolved'; request_id: string; allowed: boolean; reason?: string; persist_scope?: PersistScope; actor?: MessageOrigin }
  // Chat MCP permission card — the chat mirror of the command-guard pair for MCP
  // server tool calls. Carries the MCP server + tool identity and an args summary.
  | { type: 'McpPermissionRequested'; request_id: string; tool_use_id: string; server_id: string; server_name: string; tool_name: string; arguments_summary: string }
  | { type: 'McpPermissionResolved'; request_id: string; allowed: boolean; reason?: string; persist_scope?: PersistScope; actor?: MessageOrigin }
  // Command guard checkpoint/undo (ADR 0002, Phase 4) — a ReversibleDanger
  // command was snapshotted before running; the user can one-click Undo.
  // `restores` / `removes` are absent on events written before 2026-08-06,
  // when Undo could only restore. Treated as 0 at render, which reads as
  // "unknown", and the card then says nothing about what Undo will do.
  | { type: 'CommandCheckpointed'; checkpoint_id: string; command: string; summary: string; restores?: number; removes?: number }
  | { type: 'CommandCheckpointReverted'; checkpoint_id: string; actor?: MessageOrigin }
  | { type: 'ChildThreadCompleted'; child_thread_id: string; child_thread_title?: string; status: ChildCompletionStatus; summary: string; pending_change_ids?: string[] }
  // ── Passive / bookkeeping events ──────────────────────────────────────
  // None of these render in the timeline (EventClass::Metadata in Rust, no
  // exchange-render case, excluded from EXCHANGE_START_TYPES). They belong on
  // the union so projections / SSE handlers can pattern-match on `event.type`
  // without `as` casts, and so the union-coverage contract test stays green.
  //
  // Background worktree cleanup (Phase 10.2). tier=1 stripped build artifacts;
  // tier=2 removed the whole worktree; freed_bytes is best-effort;
  // branch_deleted is true when Tier 2 also dropped a fully-merged branch.
  | { type: 'WorktreeCleaned'; tier: number; freed_bytes: number; branch_deleted?: boolean }
  // The agent dropped a prior ToolCalled/ToolResult/ChildThreadCompleted from
  // future resume context via the `dismiss_from_context` tool.
  | { type: 'ContextDismissed'; dismissed_event_id: string }
  // The agent set one tool result's clock back to zero, by naming its address
  // under a [KEEP OPEN] heading in its working understanding. A record, never
  // the mechanism: the keep is applied where the span is parsed.
  | { type: 'ContextKeptOpen'; kept_open_event_id: string }
  // Background task lifecycle (run_bash_background / run_python_background). The
  // durable audit trail behind the bash_output / bash_kill tools; `command` is
  // the exact shell invocation. Started is paired with a later Completed.
  // On Completed, `exit_code` and `signal` are mutually exclusive: exit_code is
  // set only for a normal exit, signal only for a signal death (9 SIGKILL — also
  // the watchdog timeout and bash_kill — 11 SIGSEGV, 13 SIGPIPE). Both null means
  // the engine could not obtain a status; that is a failure, never a success.
  // `signal` is absent on rows written before the field existed.
  | { type: 'BackgroundBashStarted'; task_id: string; command: string; timeout_secs: number; started_at: string }
  | { type: 'BackgroundBashCompleted'; task_id: string; command: string; exit_code: number | null; signal?: number | null; stdout: string; stderr: string; started_at: string; finished_at: string; timed_out?: boolean; killed?: boolean }
  // Background Flash enrichment of a prior MessageReceived's attached images
  // (one event per attached hash, all carrying the same description text).
  // Replaces the deprecated `image_description` field on MessageReceived; new
  // emissions of MessageReceived no longer carry it. The frontend has no UI
  // consumer today — the description only matters to the backend's history /
  // title-generation paths — but the type belongs on the union so projections
  // can pattern-match without `as` casts when the SSE stream delivers one.
  | { type: 'ImageDescribed'; source_event_id: string; hash: string; description: string; model: string }
  | {
      type: 'ConversationSummarized';
      summary: string;
      covers_through_event_id: string;
      covered_count: number;
      model: string;
    }
  // ── Event-wait lifecycle ──────────────────────────────────────────────
  // The thread subscribed to an event and parked; the engine re-enters it on a
  // match, the deadline, or a user cancel. These DO render. `EventWaitStarted`
  // becomes an *event row* in the transcript (the marker it shares with a
  // delivery, a child callback and a trigger fire), never an exchange divider,
  // because the delivery resumes the SAME exchange. A delivery and an expiry
  // resolve that row in place by `wait_id`, adding the outcome to the same
  // subject line. A cancel never touches it: see the `EventWaitCanceled` note
  // below. They also feed `meta.liveEventWaits`, which backs the always-visible
  // waiting indicator.
  //
  // `was_attached` records whether the delivery filled in the model's own
  // dangling `await_event` tool call (a seamless mid-thought resume) or arrived
  // as a new exchange because a user message had already forced that call shut.
  | { type: 'EventWaitStarted'; wait_id: string; tool_use_id: string; on: EventSubscription[]; reason: string; armed_at?: string; expires_at: string; watermark: number }
  | { type: 'EventWaitDelivered'; wait_id: string; event_id: string; event_type: string; payload: unknown; matched_index: number; was_attached: boolean }
  | { type: 'EventWaitExpired'; wait_id: string; was_attached: boolean }
  // A stop NEVER rewrites the arming row, whoever stopped it: "Set up an event
  // wait: X" is a true statement about a moment, and a stop is a different
  // action at a different moment, routinely hours later. It renders at its own
  // position instead, as a turn when the user pressed Stop waiting and as a step
  // for every other cause.
  //
  // `on` / `reason` are a copy of what was stopped, which is what makes that
  // self-contained: a subscription routinely outlives the turn that armed it, so
  // its `EventWaitStarted` is outside the loaded window by the time it is
  // stopped. Absent on pre-2026-08-07 rows, which fall back to naming nothing.
  //
  // `actor` rides `EventMeta` like every other stamped event. A `user_stop`
  // opens a turn of its own (see `isUserStoppedWait`), and this is what puts
  // the device that pressed the button in that turn's origin popover.
  | { type: 'EventWaitCanceled'; wait_id: string; cause: EventWaitCancelCause; on?: EventSubscription[]; reason?: string; was_attached?: boolean; actor?: MessageOrigin };

/** Every `ThreadEvent['type']` discriminant, as a compile-time-checked object.
 *  The `satisfies Record<ThreadEvent['type'], true>` annotation forces this map
 *  to stay in EXACT lockstep with the union above: add a variant and `tsc`
 *  fails until you add its key here; remove one and the excess key fails. This
 *  is the only runtime-enumerable view of the union (TS types are erased), so
 *  the union-coverage contract test (`src/generated/thread-event-union.test.ts`)
 *  reads `THREAD_EVENT_TYPE_NAMES` to assert every generated
 *  `EVENT_CLASSIFICATION` entry has a matching payload member here — the guard
 *  that turns Rust→TS event drift from silent into a failing test. */
const THREAD_EVENT_TYPE_FLAGS = {
  MessageReceived: true,
  QueuedMessageRemoved: true,
  TextStreamed: true,
  ThoughtStreamed: true,
  ContextTokensMeasured: true,
  ContextAssembled: true,
  ContextCaptured: true,
  MemoryRecalled: true,
  MemorySearched: true,
  ToolCalled: true,
  ToolResult: true,
  TodoListWritten: true,
  WorkingUnderstandingWritten: true,
  ResponseGenerated: true,
  ResponseCanceled: true,
  ResponseAborted: true,
  ResponseFailed: true,
  SessionStarted: true,
  ContinuationStarted: true,
  SessionEnded: true,
  CodingAgentTextStreamed: true,
  CodingAgentThoughtStreamed: true,
  CodingAgentToolCalled: true,
  CodingAgentToolResult: true,
  CodingAgentUserMessageSent: true,
  CodingAgentPromptSent: true,
  MissingHardeningDetected: true,
  CodingAgentIdled: true,
  ContinuationRequested: true,
  ThreadTitleGenerated: true,
  ThreadTitleRenamed: true,
  ThreadSaved: true,
  ThreadUnsaved: true,
  ThreadArchived: true,
  ThreadStarted: true,
  ThreadDiscarded: true,
  ImageUploaded: true,
  TriggerStarted: true,
  TriggerCompleted: true,
  ChangeProposed: true,
  ChangeApplied: true,
  ChangeDiscarded: true,
  ChangeReverted: true,
  ChangeApplyFailed: true,
  ChangeHardened: true,
  MergeConflictDetected: true,
  MergeResolutionStarted: true,
  MergeResolutionCleared: true,
  UserPromptInjected: true,
  CredentialRequested: true,
  McpConsentRequested: true,
  CodingAgentSettingsChanged: true,
  UserQuestionAsked: true,
  UserQuestionAnswered: true,
  CodingAgentPermissionRequest: true,
  CodingAgentPermissionResolved: true,
  CommandPermissionRequested: true,
  CommandPermissionResolved: true,
  McpPermissionRequested: true,
  McpPermissionResolved: true,
  CommandCheckpointed: true,
  CommandCheckpointReverted: true,
  ChildThreadCompleted: true,
  EventWaitStarted: true,
  EventWaitDelivered: true,
  EventWaitExpired: true,
  EventWaitCanceled: true,
  WorktreeCleaned: true,
  ContextDismissed: true,
  ContextKeptOpen: true,
  BackgroundBashStarted: true,
  BackgroundBashCompleted: true,
  ImageDescribed: true,
  ConversationSummarized: true,
} satisfies Record<ThreadEvent['type'], true>;

/** Runtime-enumerable set of every `ThreadEvent['type']` discriminant. Derived
 *  from `THREAD_EVENT_TYPE_FLAGS`, so it inherits that map's compile-time
 *  exhaustiveness against the union. Consumed by the union-coverage contract
 *  test. */
export const THREAD_EVENT_TYPE_NAMES: ReadonlySet<ThreadEvent['type']> = new Set(
  Object.keys(THREAD_EVENT_TYPE_FLAGS) as ThreadEvent['type'][],
);

/** See `system-knowhow/glossary.md` § Todo item. `waiting` and `abandoned` are
 *  both engine-only: at response termination the engine settles every still-open
 *  item, to `waiting` when the thread holds a live *event wait* (it parked on
 *  purpose, the same fact the thread's own `waiting` status dot shows) and to
 *  `abandoned` otherwise (it walked away). The LLM cannot write either via
 *  `todo_write`. */
export type TodoStatus =
  | 'pending'
  | 'in_progress'
  | 'completed'
  | 'waiting'
  | 'abandoned';

/** See `system-knowhow/glossary.md` § Todo item. `active_form` is shown only
 *  while `status === 'in_progress'`; otherwise the row renders `content`. */
export interface TodoItem {
  content: string;
  active_form: string;
  status: TodoStatus;
}

/** One live *event wait* on a thread, projected into `meta.liveEventWaits` from
 *  the `EventWait*` events (see `handleEvent`).
 *
 *  Held in meta rather than re-derived per render for the same reason as
 *  `latestTodoList`: the waiting indicator is always mounted, so walking
 *  the events Map on every `threadMap` flush would cost a scan per keystroke. */
export interface EventWaitSummary {
  wait_id: string;
  on: EventSubscription[];
  reason: string;
  /** ISO-8601 deadline. The indicator counts down to it in component-local
   *  state, never in a signal: a per-second store write would re-flush
   *  `threadMap` every second for every subscribed thread. */
  expires_at: string;
}

/** Mirrors the Rust `QuestionOption` in thread_events.rs. */
export interface QuestionOption {
  id: string;
  label: string;
  description?: string;
}

/** Mirrors the Rust `TriggerInvocation` (serde tag = "kind") — records which
 *  path fired a particular trigger run. The popover panel uses this to label
 *  the run as "Scheduled" vs "Event triggered" and, for the latter, to render
 *  the matched event details. */
export type TriggerInvocation =
  | { kind: 'Schedule' }
  | { kind: 'Event'; event_type: string; event_id?: string };

/** Mirrors the Rust `AnswerKind` (serde tag = "kind"). MultiSelected's
 *  optional `text` carries freetext typed in the prompt textarea while the
 *  question was on screen — backend joins it with the resolved labels when
 *  relaying to CC. */
export type AnswerKind =
  | { kind: 'Selected'; option_id: string }
  | { kind: 'FreeText'; text: string }
  | { kind: 'MultiSelected'; option_ids: string[]; text?: string }
  | { kind: 'Canceled' }
  /** A follow-up arrived that could not be this question's answer, so it
   *  replaced the question. Distinct from `Canceled`: the user did reply, just
   *  not to this. Resolving it is what releases the agent parked inside the
   *  call that asked. */
  | { kind: 'Superseded' };

// Transient events — live SSE only, never stored. All names are past tense
// (events-only model; T7 renamed every imperative / present-participle variant).
// Old wire names (e.g. RefreshAppUI, CredentialRequest) only appear on legacy
// persisted rows, never on live SSE — old clients won't see them once the
// engine restarts, so this union only enumerates the canonical names.
export type TransientEvent =
  // Streaming state
  | { type: 'CumulativeTextUpdated'; text: string }
  | { type: 'LlmCallRetried'; reason: string }
  | { type: 'PreambleCompleted' }
  // Request events — trigger frontend modals/actions
  | { type: 'CredentialPromptRequested'; payload: string }
  | { type: 'PluginInstallRequested'; payload: string }
  | { type: 'PluginUninstallRequested'; payload: string }
  | { type: 'EmailConfirmRequested'; payload: string }
  | { type: 'PushNotificationRequested' }
  | { type: 'AppUiRefreshRequested'; app_id: string }
  | { type: 'AppUiCaptureRequested'; app_id: string; request_id: string }
  // `actor` carries the originating device for an agent (navigate_ui) navigate —
  // the device that sent the prompt that triggered the turn. The SSE handler
  // scopes the navigate to that device. Absent for trigger/background turns and
  // for the SDK app-iframe (nil-thread) path.
  | { type: 'NavigationRequested'; payload: string; actor?: MessageOrigin }
  | { type: 'CodingAgentThreadSpawned'; cc_thread_id: string; title: string; coding_agent?: CodingAgent }
  | { type: 'CodingAgentDiffChanged'; has_diff: boolean }
  | { type: 'ChildrenCountChanged'; active: number; total: number };

export type StoredEvent = ThreadEvent & { created?: string; _displayCreated?: string; _eventId?: string };

/** Events that define (or redefine) a thread's channel/source. */
export function isChannelDefiningEvent(eventType: string): boolean {
  return eventType === 'SessionStarted'
    || eventType === 'ContinuationStarted'
    || eventType === 'TriggerStarted';
}

export type SequencedEvent = {
  seq: number;
  event: StoredEvent;
};

/** Thread section as stored in the DB projection (thread_summaries.archive_state).
 *  'archived' = archive/saved, 'inbox' = needs user attention (both chat and CC).
 *  Wire JSON field name stays `section` for backwards-compat. */
export type ThreadSection = 'archived' | 'inbox';
