import type { CodingAgentChangeState } from '../../api/threads';
import {
  handleEvent,
  isSwitchTeardownAbort,
  NO_CHANGE,
  type AbortCause,
  type MessageOrigin,
  type ThreadAggregate,
  type ThreadEvent,
  type ThreadMeta,
  type ThreadState,
  type TransientEvent,
  type ThreadStatus,
} from '../thread-events';

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
    summaryVersion: Math.max(meta.summaryVersion, 0) + 1,
    activeChildrenCount: meta.activeChildrenCount,
    totalChildrenCount: meta.totalChildrenCount,
    blockingDescendantCount: meta.blockingDescendantCount,
    attentionDescendantCount: meta.attentionDescendantCount,
    liveEventWaitCount: meta.liveEventWaitCount,
    codingAgentChangeState: meta.codingAgentChangeState,
    codingAgentIsExternalRepo: meta.codingAgentIsExternalRepo,
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

function branchWork(state: CodingAgentChangeState, hasWork: boolean): CodingAgentChangeState {
  if (state.kind === 'proposed') return state;
  if (!hasWork) return NO_CHANGE;
  return state.kind === 'unproposed' ? state : { kind: 'unproposed', reason: null };
}

function changeLeft(state: CodingAgentChangeState, landing: 'none' | 'unproposed'): CodingAgentChangeState {
  if (state.kind !== 'proposed') return state;
  return landing === 'none' ? NO_CHANGE : { kind: 'unproposed', reason: null };
}

function nextChangeState(state: CodingAgentChangeState, event: ThreadEvent | TransientEvent): CodingAgentChangeState {
  switch (event.type) {
    case 'CodingAgentIdled':
      return event.has_changes === undefined ? state : branchWork(state, event.has_changes);
    case 'CodingAgentDiffChanged':
      return branchWork(state, event.has_diff);
    case 'ChangeProposed':
      if (event.set_aside) return changeLeft(state, 'unproposed');
      return { kind: 'proposed', requires_restart: event.requires_restart === true };
    case 'ChangeBroughtBack':
      return { kind: 'proposed', requires_restart: false };
    case 'ChangeSetAside':
    case 'ChangeWithdrawn':
      return changeLeft(state, 'unproposed');
    case 'ChangeApplied':
    case 'ChangeDiscarded':
      return branchWork(changeLeft(state, 'none'), false);
    case 'ThreadArchived':
      return branchWork(state, false);
    case 'ProposalWithheld':
      return state.kind === 'proposed' ? state : { kind: 'unproposed', reason: event.reason };
    case 'CodingAgentUserMessageSent':
    case 'PromptInjected':
    case 'ContinuationRequested':
      return state.kind === 'unproposed' && state.reason !== null ? { kind: 'unproposed', reason: null } : state;
    default:
      return state;
  }
}

const VERDICT_STATUSES: ReadonlySet<string> = new Set(['failed', 'paused']);

const START_EVENTS: ReadonlySet<string> = new Set([
  'MessageReceived', 'TriggerStarted', 'CodingAgentUserMessageSent',
  'PromptInjected', 'CodingAgentPromptSent', 'ContinuationRequested',
  'UserQuestionAnswered', 'CodingAgentPermissionResolved',
  'CommandPermissionResolved', 'McpPermissionResolved',
]);

const ACTIVITY_EVENTS: ReadonlySet<string> = new Set([
  'TextStreamed', 'ThoughtStreamed', 'ToolCalled', 'ToolResult', 'MemoryRecalled',
  'CodingAgentTextStreamed', 'CodingAgentThoughtStreamed',
  'CodingAgentToolCalled', 'CodingAgentToolResult',
]);

const VERDICT_PRESERVING_TERMINALS: ReadonlySet<string> = new Set([
  'ResponseCanceled', 'SessionEnded', 'CodingAgentIdled',
]);

function applyEventRules(agg: ThreadAggregate, event: ThreadEvent | TransientEvent): ThreadAggregate {
  const out: ThreadAggregate = { ...agg };
  const t = event.type;

  if (t === 'SessionEnded') {
    const reason = (event as { reason?: string }).reason;
    if (reason === 'stale_resume') return out;
    if (reason === 'discarded') {
      out.codingAgentChangeState = NO_CHANGE;
      out.codingAgentIsExternalRepo = false;
      out.status = 'idle';
      return out;
    }
  }

  const setRunning: ReadonlySet<string> = new Set([
    ...START_EVENTS,
    ...ACTIVITY_EVENTS,
  ]);
  const setIdle: ReadonlySet<string> = new Set([
    'TriggerCompleted', 'ChangeApplied', 'ChangeDiscarded', 'ThreadArchived',
    'ResponseGenerated', 'ResponseCanceled', 'SessionEnded', 'CodingAgentIdled',
  ]);

  out.codingAgentChangeState = nextChangeState(agg.codingAgentChangeState, event);
  if (t === 'ChangeApplied' || t === 'ChangeDiscarded' || t === 'ThreadArchived') {
    out.codingAgentIsExternalRepo = false;
  }

  if (
    VERDICT_STATUSES.has(out.status)
    && (ACTIVITY_EVENTS.has(t) || VERDICT_PRESERVING_TERMINALS.has(t))
  ) {
    return out;
  }

  if (setRunning.has(t)) out.status = 'running';
  else if (setIdle.has(t)) out.status = 'idle';
  else if (t === 'ResponseFailed') out.status = 'failed';
  else if (t === 'ResponseAborted') {
    const { cause, actor } = event as { cause?: AbortCause; actor?: MessageOrigin };
    out.status = cause === 'stale_settle' ? 'idle'
      : isSwitchTeardownAbort(actor, cause) ? 'paused' as ThreadStatus
      : 'failed';
  }
  else if (t === 'UserQuestionAsked' || t === 'CodingAgentPermissionRequest' || t === 'CommandPermissionRequested' || t === 'McpPermissionRequested') out.status = 'waiting_for_user_answer' as ThreadStatus;

  return out;
}

function synthesizeAggregate(
  meta: ThreadMeta,
  event: ThreadEvent | TransientEvent,
  overrides: Partial<ThreadAggregate> = {},
): ThreadAggregate {
  const base = aggregateFromMeta(meta);
  const withRules = applyEventRules(base, event);
  return { ...withRules, ...overrides };
}

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
    return handleEvent(threadMap, threadId, seq, event, created, eventId).applied;
  }
  const overrides: Partial<ThreadAggregate> =
    created ? { lastActivity: created, ...aggregateOverrides } : aggregateOverrides;
  const agg = synthesizeAggregate(thread.meta, event, overrides);
  return handleEvent(threadMap, threadId, seq, event, created, eventId, agg).applied;
}
