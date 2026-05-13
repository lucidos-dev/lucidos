// Test-only helper that synthesizes a ThreadAggregate snapshot for each
// event passed to handleEvent. handleEvent applies aggregates rather than
// deriving meta from event types, so unit tests that exercise integration
// flows (exchanges, groupings, optimistic UI) need a stand-in for the
// backend's projection. The Rust lifecycle tests
// (engine/thread_lifecycle_tests/tests.rs) remain the source of truth for
// the rules; this helper is a TS-side mirror, not a duplicate spec.
import {
  applyAggregateToMeta,
  handleEvent,
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
    ccHasChanges: meta.ccHasChanges,
    ccRequiresRestart: meta.ccRequiresRestart,
    ccIsExternalRepo: meta.ccIsExternalRepo,
    ccApplying: meta.ccApplying,
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

/** Apply the status/cc-flag rule for `event` to `agg`, returning a new
 *  aggregate. Mirrors thread_lifecycle.rs::status_transitions(). */
function applyEventRules(agg: ThreadAggregate, event: ThreadEvent | TransientEvent): ThreadAggregate {
  const out: ThreadAggregate = { ...agg };
  const t = event.type;

  // SessionEnded special cases — payload-dependent, mirrors the deleted
  // updateStatusFromEvent's handling.
  if (t === 'SessionEnded') {
    const reason = (event as { reason?: string }).reason;
    // Stale resume is mid-flight retry — backend skips status update entirely.
    if (reason === 'stale_resume') return out;
    // Discarded clears all CC flags and forces idle.
    if (reason === 'discarded') {
      out.ccHasChanges = false;
      out.ccRequiresRestart = false;
      out.ccIsExternalRepo = false;
      out.ccApplying = false;
      out.status = 'idle';
      return out;
    }
  }

  // Status rules
  const setRunning: ReadonlySet<string> = new Set([
    'MessageReceived', 'TriggerStarted', 'CodingAgentUserMessageSent',
    'UserPromptInjected', 'CodingAgentPromptSent', 'ContinueSignal',
    'TextStreamed', 'Thinking', 'ToolCalled', 'ToolResult',
    'CodingAgentTextStreamed', 'CodingAgentToolCalled', 'CodingAgentToolResult',
    'UserQuestionAnswered', 'CodingAgentPermissionResolved',
  ]);
  const setIdle: ReadonlySet<string> = new Set([
    'TriggerCompleted', 'ChangeApplied', 'ChangeDiscarded', 'ThreadArchived',
  ]);
  // ConditionalCc(Waiting, Idle) — Waiting if ccHasChanges, else Idle
  const conditionalWaitingIdle: ReadonlySet<string> = new Set([
    'ResponseGenerated', 'ResponseCanceled', 'SessionEnded', 'CodingAgentIdled',
  ]);

  // CC flag rules — applied first so conditional_cc sees the updated state.
  if (event.type === 'CodingAgentIdled') {
    if (event.has_changes !== undefined) out.ccHasChanges = !!event.has_changes;
    if (event.requires_restart !== undefined) out.ccRequiresRestart = !!event.requires_restart;
  } else if (t === 'ChangeProposed') {
    out.ccHasChanges = true;
  } else if (t === 'ChangeApplied' || t === 'ChangeDiscarded' || t === 'ThreadArchived') {
    out.ccHasChanges = false;
    out.ccRequiresRestart = false;
    out.ccIsExternalRepo = false;
    out.ccApplying = false;
  } else if (t === 'MergeConflictDetected') {
    out.ccApplying = true;
  } else if (t === 'ChangeApplyFailed') {
    out.ccApplying = false;
  }

  if (setRunning.has(t)) out.status = 'running';
  else if (setIdle.has(t)) out.status = 'idle';
  else if (t === 'ResponseFailed') out.status = 'failed';
  else if (t === 'ResponseAborted') out.status = out.ccHasChanges ? 'waiting' : 'failed';
  else if (t === 'UserQuestionAsked' || t === 'CodingAgentPermissionRequest') out.status = 'waiting_for_user_answer' as ThreadStatus;
  else if (conditionalWaitingIdle.has(t)) out.status = out.ccHasChanges ? 'waiting' : 'idle';

  return out;
}

/** Synthesize the post-event aggregate that the backend would compute. */
export function synthesizeAggregate(
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
    return handleEvent(threadMap, threadId, seq, event, created, eventId);
  }
  // Stamp lastActivity from the event timestamp so the aggregate doesn't
  // overwrite the server-time updatedAt that handleEvent has already set.
  const overrides: Partial<ThreadAggregate> =
    created ? { lastActivity: created, ...aggregateOverrides } : aggregateOverrides;
  const agg = synthesizeAggregate(thread.meta, event, overrides);
  return handleEvent(threadMap, threadId, seq, event, created, eventId, agg);
}

export { applyAggregateToMeta };
