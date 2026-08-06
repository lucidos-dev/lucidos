// Test-only helper that synthesizes a ThreadAggregate snapshot for each
// event passed to handleEvent. handleEvent applies aggregates rather than
// deriving meta from event types, so unit tests that exercise integration
// flows (exchanges, groupings, optimistic UI) need a stand-in for the
// backend's projection. The Rust lifecycle tests
// (engine/thread_lifecycle_tests/tests.rs) remain the source of truth for
// the rules; this helper is a TS-side mirror, not a duplicate spec.
import {
  handleEvent,
  isSwitchTeardownAbort,
  type AbortCause,
  type MessageOrigin,
  type ThreadAggregate,
  type ThreadEvent,
  type ThreadMeta,
  type ThreadState,
  type TransientEvent,
  type ThreadStatus,
} from '../thread-events';

/** Build a ThreadAggregate snapshot from the current meta. Used as the
 *  starting point before applying per-event rules. */
function aggregateFromMeta(meta: ThreadMeta): ThreadAggregate {
  if (meta.channel === 'error_unknown_channel') {
    throw new Error(`aggregateFromMeta: thread ${meta.id} has channel='error_unknown_channel' — fix the test setup, don't coerce a sentinel into 'chat'`);
  }
  return {
    threadId: meta.id,
    title: meta.title,
    channel: meta.channel,
    initiator: meta.initiator,
    createdAt: meta.createdAt,
    lastActivity: meta.updatedAt,
    messageCount: meta.messageCount,
    section: meta.section,
    status: meta.status,
    activeChildrenCount: meta.activeChildrenCount,
    totalChildrenCount: meta.totalChildrenCount,
    blockingDescendantCount: meta.blockingDescendantCount,
    attentionDescendantCount: meta.attentionDescendantCount,
    codingAgentProposed: meta.codingAgentProposed,
    codingAgentRequiresRestart: meta.codingAgentRequiresRestart,
    codingAgentIsExternalRepo: meta.codingAgentIsExternalRepo,
    codingAgentApplying: meta.codingAgentApplying,
    codingAgentHasDiff: meta.codingAgentHasDiff,
    isSaved: meta.saved,
    hasResponse: false,
    lastRevivedAt: meta.lastRevivedAt || null,
    parentThreadId: meta.parentThreadId ?? null,
    parentThreadTitle: meta.parentThreadTitle ?? null,
    triggerId: meta.triggerId,
    triggerName: meta.triggerName,
    ccRepoId: meta.repoId,
    ccRepoName: meta.repoName,
    state: meta.state,
  };
}

/** Status values that are a VERDICT about how the turn ended, not a resting
 *  state. Mirrors `PRESERVED_STATUS_VERDICTS` (engine `event_bus/mod.rs`). */
const VERDICT_STATUSES: ReadonlySet<string> = new Set(['failed', 'paused']);

/** Events that mean NEW WORK was requested, so they clear a verdict. Their
 *  projection arms write `status = 'running'` plainly. */
const START_EVENTS: ReadonlySet<string> = new Set([
  'MessageReceived', 'TriggerStarted', 'CodingAgentUserMessageSent',
  'UserPromptInjected', 'CodingAgentPromptSent', 'ContinuationRequested',
  'UserQuestionAnswered', 'CodingAgentPermissionResolved',
  'CommandPermissionResolved', 'McpPermissionResolved',
]);

/** Events that merely stream a turn's output. Their projection arm writes
 *  `preserving_verdict("'running'")`, so they revive a thread that drifted to
 *  idle/waiting but never one carrying a verdict. */
const ACTIVITY_EVENTS: ReadonlySet<string> = new Set([
  'TextStreamed', 'ThoughtStreamed', 'ToolCalled', 'ToolResult', 'MemorySearched',
  'CodingAgentTextStreamed', 'CodingAgentThoughtStreamed',
  'CodingAgentToolCalled', 'CodingAgentToolResult',
]);

/** Events that merely CLOSE OUT an ended turn, so they must not overwrite a
 *  verdict either. Their arms write `preserving_verdict(STATUS_FROM_PROPOSED_CHANGE)`.
 *  `ResponseGenerated` is deliberately absent: its arm writes the bare
 *  `STATUS_FROM_PROPOSED_CHANGE`, because a turn that generated a response
 *  really did finish. */
const VERDICT_PRESERVING_TERMINALS: ReadonlySet<string> = new Set([
  'ResponseCanceled', 'SessionEnded', 'CodingAgentIdled',
]);

/** Apply the status/cc-flag rule for `event` to `agg`, returning a new
 *  aggregate. Mirrors thread_lifecycle.rs::status_transitions(), plus the
 *  projection's `preserving_verdict` guard, which the contract table cannot
 *  express (see the note on `CodingAgentIdled` there).
 *
 *  The guard is load-bearing for anything replaying a coding-agent teardown: a
 *  dying subprocess keeps draining `CodingAgentTextStreamed` /
 *  `CodingAgentToolResult` for milliseconds after the abort, and then emits
 *  `CodingAgentIdled` / `SessionEnded`. Without the mirror, this helper walked
 *  a just-interrupted thread from 'paused' back to 'running' and then to
 *  'idle', so a test could not reproduce what the client actually receives. */
function applyEventRules(agg: ThreadAggregate, event: ThreadEvent | TransientEvent): ThreadAggregate {
  const out: ThreadAggregate = { ...agg };
  const t = event.type;

  // SessionEnded special cases: payload-dependent, mirroring the deleted
  // updateStatusFromEvent's handling. Both return before the
  // `preserving_verdict` guard below, deliberately. `stale_resume` writes no
  // status at all, and `discarded` is a user action, which is the one kind of
  // thing that may overwrite a verdict.
  if (t === 'SessionEnded') {
    const reason = (event as { reason?: string }).reason;
    // Stale resume is a mid-flight retry, so the backend skips the status update.
    if (reason === 'stale_resume') return out;
    // Discarded clears all CC flags and forces idle. It covers the stale-session
    // discard where no pending change exists, so no `ChangeDiscarded` is emitted
    // to carry the clear (see the `thread-flows-cc-status` case for that shape).
    if (reason === 'discarded') {
      out.codingAgentProposed = false;
      out.codingAgentRequiresRestart = false;
      out.codingAgentIsExternalRepo = false;
      out.codingAgentApplying = false;
      out.status = 'idle';
      return out;
    }
  }

  // Status rules
  const setRunning: ReadonlySet<string> = new Set([
    ...START_EVENTS,
    ...ACTIVITY_EVENTS,
  ]);
  const setIdle: ReadonlySet<string> = new Set([
    'TriggerCompleted', 'ChangeApplied', 'ChangeDiscarded', 'ThreadArchived',
  ]);
  // ConditionalCc(Waiting, Idle) — Waiting if codingAgentProposed, else Idle
  const conditionalWaitingIdle: ReadonlySet<string> = new Set([
    'ResponseGenerated', 'ResponseCanceled', 'SessionEnded', 'CodingAgentIdled',
  ]);

  // CC flag rules — applied first so conditional_cc sees the updated state.
  if (event.type === 'CodingAgentIdled') {
    if (event.has_changes !== undefined) out.codingAgentProposed = !!event.has_changes;
    if (event.requires_restart !== undefined) out.codingAgentRequiresRestart = !!event.requires_restart;
  } else if (t === 'ChangeProposed') {
    out.codingAgentProposed = true;
  } else if (t === 'ChangeApplied' || t === 'ChangeDiscarded' || t === 'ThreadArchived') {
    out.codingAgentProposed = false;
    out.codingAgentRequiresRestart = false;
    out.codingAgentIsExternalRepo = false;
    out.codingAgentApplying = false;
  } else if (t === 'MergeConflictDetected') {
    out.codingAgentApplying = true;
  } else if (t === 'ChangeApplyFailed') {
    out.codingAgentApplying = false;
  }

  // `preserving_verdict`: an event that only streams or closes out the ended
  // turn leaves a 'failed' / 'paused' verdict exactly where it is.
  if (
    VERDICT_STATUSES.has(out.status)
    && (ACTIVITY_EVENTS.has(t) || VERDICT_PRESERVING_TERMINALS.has(t))
  ) {
    return out;
  }

  if (setRunning.has(t)) out.status = 'running';
  else if (setIdle.has(t)) out.status = 'idle';
  else if (t === 'ResponseFailed') out.status = 'failed';
  // Mirrors `AbortCause::status_sql()`, which reads the ACTOR as well as the
  // cause: only the user's own switch teardown (`isSwitchTeardownAbort`) settles
  // at 'paused', because that is the one interruption the engine promised to
  // resume. Every other abort is 'failed', except 'stale_settle' at the
  // cancel-style idle/waiting. Pending changes override every arm.
  else if (t === 'ResponseAborted') {
    const { cause, actor } = event as { cause?: AbortCause; actor?: MessageOrigin };
    out.status = out.codingAgentProposed
      ? 'waiting'
      : cause === 'stale_settle' ? 'idle'
      : isSwitchTeardownAbort(actor, cause) ? 'paused' as ThreadStatus
      : 'failed';
  }
  else if (t === 'UserQuestionAsked' || t === 'CodingAgentPermissionRequest' || t === 'CommandPermissionRequested' || t === 'McpPermissionRequested') out.status = 'waiting_for_user_answer' as ThreadStatus;
  else if (conditionalWaitingIdle.has(t)) out.status = out.codingAgentProposed ? 'waiting' : 'idle';

  return out;
}

/** Synthesize the post-event aggregate that the backend would compute. */
function synthesizeAggregate(
  meta: ThreadMeta,
  event: ThreadEvent | TransientEvent,
  overrides: Partial<ThreadAggregate> = {},
): ThreadAggregate {
  const base = aggregateFromMeta(meta);
  const withRules = applyEventRules(base, event);
  return { ...withRules, ...overrides };
}

/** Drop-in replacement for handleEvent that synthesizes the aggregate the
 *  backend would have shipped, so integration tests see status/CC-flag
 *  updates as a side effect of replaying events. */
export function handleEventWithAgg(
  threadMap: Map<string, ThreadState>,
  threadId: string,
  seq: number | null,
  event: ThreadEvent | TransientEvent,
  created?: string,
  eventId?: string,
  aggregateOverrides: Partial<ThreadAggregate> = {},
): boolean {
  const thread = threadMap.get(threadId);
  if (!thread || seq === null) {
    // Transient events don't carry aggregates; defer to plain handleEvent.
    return handleEvent(threadMap, threadId, seq, event, created, eventId).applied;
  }
  // Stamp lastActivity from the event timestamp so the aggregate doesn't
  // overwrite the server-time updatedAt that handleEvent has already set.
  const overrides: Partial<ThreadAggregate> =
    created ? { lastActivity: created, ...aggregateOverrides } : aggregateOverrides;
  const agg = synthesizeAggregate(thread.meta, event, overrides);
  return handleEvent(threadMap, threadId, seq, event, created, eventId, agg).applied;
}
