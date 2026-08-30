import type {
  AbortCause,
  ActorMode,
  EventSubscription,
  MessageOrigin,
  ThreadEvent,
} from '../../generated/thread-event-wire';

// The wire types are GENERATED from the Rust event source
// (`thread_events_tests/ts_codegen.rs`). Re-exported here so every consumer
// keeps one import path for "what an event looks like", and so a payload
// field can no longer be hand-mirrored. What stays hand-written in this file
// is everything the wire does not decide: the display labels, the summary
// helpers, and the fingerprints the transcript reads.
export type {
  AbortCause,
  ActorMode,
  AgentParticipant,
  AllowScope,
  AnswerKind,
  CancelCause,
  ChildCompletionStatus,
  EngineReason,
  EventChannel,
  EventSubscription,
  EventWaitCancelCause,
  MessageOrigin,
  QuestionOption,
  SessionEndReason,
  ThreadDirection,
  ThreadEvent,
  TodoItem,
  TodoStatus,
  TransientEvent,
  TriggerInvocation,
  VoiceSessionEndReason,
} from '../../generated/thread-event-wire';
export { THREAD_EVENT_TYPE_NAMES } from '../../generated/thread-event-wire';

export type ThreadInitiator = 'user' | 'system';

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
    // A guest collapses to plain agent mode on purpose, so `actorInitiator`
    // draws one Lucidos chip whoever wrote the turn. The user meets one
    // entity (ADR 0149); the split is for `history.rs`, whose reader must
    // NOT (ADR 0150).
    case 'agent':       return 'agent';
    case 'webhook':     return 'engine';
    case 'engine':      return 'engine';
    case 'system':      return 'engine';
  }
}

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
