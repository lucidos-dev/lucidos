import { changeWork, EVENT_CLASSIFICATION, LAST_ACTIVITY_EVENTS } from '../../generated/thread-lifecycle';
import type { ChangeWork, EventChannel, ThreadStatus } from '../../generated/thread-lifecycle';
import type { EventWaitSummary, StoredEvent, ThreadInitiator, ThreadSection, TodoItem } from './thread-event-types';
import type { CardSize, CodingAgentChangeState, PairStamp, SuccessorLink, ThreadHoldSummary } from '../../api/threads';
import type { CacheKeepAliveStatus, CodingAgent } from '../../generated/thread-event-wire';

export type ThreadAggregate = {
  threadId: string;
  title: string;
  channel: string;
  initiator: ThreadInitiator;
  createdAt: string;
  lastActivity: string;
  lastUserAction?: string;
  lastAgentAction?: string;
  messageCount: number;
  section: ThreadSection;
  status: ThreadStatus;
  summaryVersion: number;
  activeChildrenCount: number;
  awaitedChildrenCount?: number;
  awaitedByParent?: boolean;
  totalChildrenCount: number;
  blockingDescendantCount: number;
  attentionDescendantCount: number;
  archivableSubThreadCount: number;
  isStoppedChild?: boolean;
  hold?: ThreadHoldSummary;
  readRequested?: boolean;
  replySeenDevices?: string[];
  applyInFlight?: boolean;
  liveEventWaitCount: number;
  liveEventWaits?: EventWaitSummary[];
  cacheKeepAliveStatus?: CacheKeepAliveStatus | null;
  codingAgentChangeState: CodingAgentChangeState;
  codingAgentIsExternalRepo: boolean;
  isSaved: boolean;
  pinFolder?: string;
  home?: true;
  hasResponse: boolean;
  lastRevivedAt: string | null;
  parentThreadId: string | null;
  parentThreadTitle: string | null;
  successor?: SuccessorLink;
  successorOf?: SuccessorLink;
  triggerId?: string;
  triggerName?: string;
  ccRepoId?: string;
  ccRepoName?: string;
  codingAgentKind?: 'lucidos' | 'app' | 'external';
  codingAgentFolder?: string;
  codingAgent?: 'claude-code' | 'codex';
  codingAgents?: CodingAgent[];
  state: ThreadComposeState;
};

function sameDevices(a: string[] | undefined, b: string[] | undefined): boolean {
  const prev = a ?? [];
  const next = b ?? [];
  return prev.length === next.length && next.every((device, i) => prev[i] === device);
}

function sameWaits(a: EventWaitSummary[], b: EventWaitSummary[]): boolean {
  return a.length === b.length && a.every((wait, i) => {
    const other = b[i];
    const timing = wait.background_timing;
    return wait.wait_id === other.wait_id
      && wait.reason === other.reason
      && wait.expires_at === other.expires_at
      && timing?.state === other.background_timing?.state
      && (timing?.state !== 'running'
        || (other.background_timing?.state === 'running'
          && timing.deadline === other.background_timing.deadline));
  });
}

export function sameCodingAgents(a: CodingAgent[] | undefined, b: CodingAgent[] | undefined): boolean {
  const prev = a ?? [];
  const next = b ?? [];
  return prev.length === next.length && next.every((agent, i) => prev[i] === agent);
}

export const UNVERSIONED = -1;

export function isSummaryCurrent(meta: ThreadMeta, version: number): boolean {
  return !(version < meta.summaryVersion);
}

export function applySummaryVersion(meta: ThreadMeta, version: number, status: ThreadStatus): boolean {
  if (!isSummaryCurrent(meta, version)) return false;
  const writable = meta as { status: ThreadStatus; summaryVersion: number };
  writable.summaryVersion = version;
  writable.status = status;
  return true;
}

function sameHold(a: ThreadHoldSummary | null | undefined, b: ThreadHoldSummary | undefined): boolean {
  return JSON.stringify(a ?? null) === JSON.stringify(b ?? null);
}

function sameKeepAliveStatus(a: CacheKeepAliveStatus | null | undefined, b: CacheKeepAliveStatus | null): boolean {
  return JSON.stringify(a ?? null) === JSON.stringify(b);
}

export function applyAggregateToMeta(meta: ThreadMeta, agg: ThreadAggregate): boolean {
  const prevStatus = meta.status;
  if (!applySummaryVersion(meta, agg.summaryVersion, agg.status)) return false;
  let changed = meta.status !== prevStatus;
  if (meta.section !== agg.section) { meta.section = agg.section; changed = true; }
  if (meta.activeChildrenCount !== agg.activeChildrenCount) { meta.activeChildrenCount = agg.activeChildrenCount; changed = true; }
  if ((meta.awaitedChildrenCount ?? 0) !== (agg.awaitedChildrenCount ?? 0)) { meta.awaitedChildrenCount = agg.awaitedChildrenCount ?? 0; changed = true; }
  if (!!meta.awaitedByParent !== !!agg.awaitedByParent) { meta.awaitedByParent = !!agg.awaitedByParent; changed = true; }
  if (meta.totalChildrenCount !== agg.totalChildrenCount) { meta.totalChildrenCount = agg.totalChildrenCount; changed = true; }
  if (meta.blockingDescendantCount !== agg.blockingDescendantCount) { meta.blockingDescendantCount = agg.blockingDescendantCount; changed = true; }
  if (meta.attentionDescendantCount !== agg.attentionDescendantCount) { meta.attentionDescendantCount = agg.attentionDescendantCount; changed = true; }
  if (meta.archivableSubThreadCount !== agg.archivableSubThreadCount) { meta.archivableSubThreadCount = agg.archivableSubThreadCount; changed = true; }
  if (!!meta.isStoppedChild !== !!agg.isStoppedChild) { meta.isStoppedChild = !!agg.isStoppedChild; changed = true; }
  if (!sameHold(meta.hold, agg.hold)) { meta.hold = agg.hold ?? null; changed = true; }
  if (!!meta.readRequested !== !!agg.readRequested) { meta.readRequested = !!agg.readRequested; changed = true; }
  if (!sameDevices(meta.replySeenDevices, agg.replySeenDevices)) { meta.replySeenDevices = agg.replySeenDevices ?? []; changed = true; }
  if (!!meta.applyInFlight !== !!agg.applyInFlight) { meta.applyInFlight = !!agg.applyInFlight; changed = true; }
  if (meta.liveEventWaitCount !== agg.liveEventWaitCount) { meta.liveEventWaitCount = agg.liveEventWaitCount; changed = true; }
  if (agg.liveEventWaits !== undefined && !sameWaits(meta.liveEventWaits, agg.liveEventWaits)) { meta.liveEventWaits = agg.liveEventWaits; changed = true; }
  if (agg.cacheKeepAliveStatus !== undefined && !sameKeepAliveStatus(meta.cacheKeepAliveStatus, agg.cacheKeepAliveStatus)) { meta.cacheKeepAliveStatus = agg.cacheKeepAliveStatus; changed = true; }
  if (!sameChangeState(meta.codingAgentChangeState, agg.codingAgentChangeState)) { meta.codingAgentChangeState = agg.codingAgentChangeState; changed = true; }
  if (meta.codingAgentIsExternalRepo !== agg.codingAgentIsExternalRepo) { meta.codingAgentIsExternalRepo = agg.codingAgentIsExternalRepo; changed = true; }
  if (meta.saved !== agg.isSaved) { meta.saved = agg.isSaved; changed = true; }
  const nextPinFolder = agg.pinFolder ?? null;
  if ((meta.pinFolder ?? null) !== nextPinFolder) { meta.pinFolder = nextPinFolder; changed = true; }
  if (agg.home && !meta.home) { meta.home = true; changed = true; }
  meta.messageCount = agg.messageCount;
  meta.updatedAt = agg.lastActivity;
  if (agg.lastUserAction !== undefined && meta.lastUserAction !== agg.lastUserAction) { meta.lastUserAction = agg.lastUserAction; changed = true; }
  if (agg.lastAgentAction !== undefined) meta.lastAgentAction = agg.lastAgentAction;
  const nextLastRevived = agg.lastRevivedAt ?? '';
  if (meta.lastRevivedAt !== nextLastRevived) { meta.lastRevivedAt = nextLastRevived; changed = true; }
  if (meta.state !== agg.state) { meta.state = agg.state; changed = true; }
  const nextParentId = agg.parentThreadId ?? undefined;
  if (meta.parentThreadId !== nextParentId) { meta.parentThreadId = nextParentId; changed = true; }
  const nextParentTitle = agg.parentThreadTitle ?? undefined;
  if (meta.parentThreadTitle !== nextParentTitle) { meta.parentThreadTitle = nextParentTitle; changed = true; }
  if (agg.successor && !sameSuccessorLink(meta.successor, agg.successor)) { meta.successor = agg.successor; changed = true; }
  if (agg.successorOf && !sameSuccessorLink(meta.successorOf, agg.successorOf)) { meta.successorOf = agg.successorOf; changed = true; }
  if (agg.triggerId && meta.triggerId !== agg.triggerId) { meta.triggerId = agg.triggerId; changed = true; }
  if (agg.triggerName && meta.triggerName !== agg.triggerName) { meta.triggerName = agg.triggerName; changed = true; }
  if (agg.ccRepoId && meta.repoId !== agg.ccRepoId) { meta.repoId = agg.ccRepoId; changed = true; }
  if (agg.ccRepoName && meta.repoName !== agg.ccRepoName) { meta.repoName = agg.ccRepoName; changed = true; }
  if (agg.codingAgentKind && meta.codingAgentKind !== agg.codingAgentKind) {
    meta.codingAgentKind = agg.codingAgentKind;
    changed = true;
  }
  if (agg.codingAgentFolder && meta.codingAgentFolder !== agg.codingAgentFolder) {
    meta.codingAgentFolder = agg.codingAgentFolder;
    changed = true;
  }
  if (agg.codingAgent && meta.codingAgent !== agg.codingAgent) {
    meta.codingAgent = agg.codingAgent;
    changed = true;
  }
  if (agg.codingAgents && !sameCodingAgents(meta.codingAgents, agg.codingAgents)) {
    meta.codingAgents = agg.codingAgents;
    changed = true;
  }
  return changed;
}

export function firstSendUnsettled(thread: ThreadState, state: ThreadMeta['state']): boolean {
  return state === 'composing'
    && (thread.pendingUserMessages.length > 0 || (thread.unsentMessageSeqs?.size ?? 0) > 0);
}

export function applyAggregateToThread(thread: ThreadState, aggregate: ThreadAggregate): boolean {
  return applyAggregateToMeta(thread.meta, firstSendUnsettled(thread, aggregate.state)
    ? { ...aggregate, state: thread.meta.state }
    : aggregate);
}

export const PENDING_TITLE_PLACEHOLDER = '...';

export type ThreadMeta = {
  id: string;
  title: string;
  channel: EventChannel | 'error_unknown_channel';
  initiator: ThreadInitiator;
  saved: boolean;
  pinFolder?: string | null;
  home?: true;
  createdAt: string;
  updatedAt: string;
  lastUserAction?: string;
  lastAgentAction?: string;
  readonly status: ThreadStatus;
  readonly summaryVersion: number;
  messageCount: number;
  section: ThreadSection;
  activeChildrenCount: number;
  awaitedChildrenCount?: number;
  awaitedByParent?: boolean;
  totalChildrenCount: number;
  blockingDescendantCount: number;
  attentionDescendantCount: number;
  archivableSubThreadCount: number;
  isStoppedChild?: boolean;
  hold?: ThreadHoldSummary | null;
  readRequested?: boolean;
  replySeenDevices?: string[];
  applyInFlight?: boolean;
  liveEventWaitCount: number;
  codingAgentChangeState: CodingAgentChangeState;
  codingAgentIsExternalRepo: boolean;
  lastRevivedAt: string;
  parentThreadId?: string;
  parentThreadTitle?: string;
  successor?: SuccessorLink;
  successorOf?: SuccessorLink;
  triggerId?: string;
  triggerName?: string;
  repoId?: string;
  repoName?: string;
  codingAgentKind?: 'lucidos' | 'app' | 'external';
  codingAgentFolder?: string;
  codingAgent?: 'claude-code' | 'codex';
  codingAgents?: CodingAgent[];
  state: ThreadComposeState;
  latestTodoList: TodoItem[] | null;
  latestTodoNotes?: string | null;
  liveEventWaits: EventWaitSummary[];
  cacheKeepAliveStatus?: CacheKeepAliveStatus | null;
};

export type ThreadComposeState = 'composing' | 'active' | 'discarded';
export type ComposeChannelMode = 'lucidos' | 'claude_code' | null;

export type PendingUserMessage = {
  text: string;
  eventId: string;
  created: string;
  image_hashes?: string[];
  unconfirmed?: boolean;
  answersQuestion?: string;
};

export type ThreadState = {
  meta: ThreadMeta;
  events: Map<number, StoredEvent>;
  unsentMessageSeqs?: Map<string, number>;
  keepAliveAnchorIds?: Set<string>;
  streamingBuffer: string;
  eventsLoaded: boolean;
  eventsLoadFailed: boolean;
  lastDbSeq: number;
  historyFloor?: { created: string; sequence: number } | null;
  hasOlderEvents?: boolean;
  pairStamps?: PairStamp[];
  cardSizes?: CardSize[];
  pendingUserMessages: PendingUserMessage[];
  liveUtterances?: LiveUtterance[];
  unclaimedUtterances?: string[];
  liveReply?: LiveReply;
};

export type LiveUtterance = {
  eventId: string;
  count: number;
  created: string;
  text?: string;
  partial?: string;
};

export type LiveReply = {
  eventId: string;
  created: string;
  text: string;
};

export function makeOptimisticThreadState(opts: {
  id: string;
  title: string;
  channel: ThreadMeta['channel'];
  initiator: ThreadInitiator;
  eventsLoaded: boolean;
  timestamp?: string;
  pendingUserMessages?: ThreadState['pendingUserMessages'];
  triggerId?: string;
  triggerName?: string;
  repoId?: string;
  repoName?: string;
  codingAgentKind?: 'lucidos' | 'app' | 'external';
  codingAgentFolder?: string;
  codingAgent?: 'claude-code' | 'codex';
  state?: ThreadComposeState;
  status?: ThreadStatus;
}): ThreadState {
  const ts = opts.timestamp ?? new Date().toISOString();
  return {
    meta: {
      id: opts.id,
      title: opts.title,
      channel: opts.channel,
      initiator: opts.initiator,
      saved: false,
      createdAt: ts,
      updatedAt: ts,
      lastUserAction: ts,
      lastAgentAction: ts,
      status: opts.status ?? 'running',
      summaryVersion: UNVERSIONED,
      messageCount: 0,
      section: 'inbox',
      activeChildrenCount: 0,
      totalChildrenCount: 0,
      blockingDescendantCount: 0,
      attentionDescendantCount: 0,
      archivableSubThreadCount: 0,
      liveEventWaitCount: 0,
      codingAgentChangeState: NO_CHANGE,
      codingAgentIsExternalRepo: false,
      lastRevivedAt: ts,
      triggerId: opts.triggerId,
      triggerName: opts.triggerName,
      repoId: opts.repoId,
      repoName: opts.repoName,
      codingAgentKind: opts.codingAgentKind,
      codingAgentFolder: opts.codingAgentFolder,
      codingAgent: opts.codingAgent,
      state: opts.state ?? 'active',
      latestTodoList: null,
      latestTodoNotes: null,
      liveEventWaits: [],
    },
    events: new Map(),
    streamingBuffer: '',
    eventsLoaded: opts.eventsLoaded,
    eventsLoadFailed: false,
    lastDbSeq: 0,
    historyFloor: null,
    hasOlderEvents: false,
    pendingUserMessages: opts.pendingUserMessages ?? [],
    liveUtterances: [],
  };
}

export const recencyKey = (t: ThreadState): string =>
  t.meta.lastUserAction || t.meta.updatedAt;

export const byRecent = (a: ThreadState, b: ThreadState): number =>
  recencyKey(b).localeCompare(recencyKey(a));

export const createdKey = (t: ThreadState): string =>
  t.meta.createdAt || t.meta.updatedAt;

export const byCreated = (a: ThreadState, b: ThreadState): number =>
  createdKey(b).localeCompare(createdKey(a));

export const isExcludedFromSections = (t: ThreadState): boolean =>
  t.meta.state === 'composing' || t.meta.state === 'discarded';

export function reviewTier(t: ThreadState, status: ThreadStatus): 0 | 1 | 2 {
  if (status === 'waiting_for_user_answer') return 0;
  if (changeReadyToReview(t.meta) || status === 'failed' || status === 'paused') return 1;
  return 2;
}

export function changeReadyToReview(meta: Pick<ThreadMeta, 'codingAgentChangeState'>): boolean {
  return meta.codingAgentChangeState.kind === 'proposed';
}

export const NO_CHANGE: CodingAgentChangeState = Object.freeze({ kind: 'none' });

export function sameChangeState(a: CodingAgentChangeState, b: CodingAgentChangeState): boolean {
  if (a === b) return true;
  switch (a.kind) {
    case 'none': return b.kind === 'none';
    case 'unproposed': return b.kind === 'unproposed' && a.reason === b.reason;
    case 'proposed': return b.kind === 'proposed' && a.requires_restart === b.requires_restart;
  }
}

export function sameSuccessorLink(a: SuccessorLink | undefined, b: SuccessorLink): boolean {
  return a?.thread_id === b.thread_id && a.title === b.title;
}

export function updatesLastActivity(type: string): boolean {
  return LAST_ACTIVITY_EVENTS.has(type);
}

export const CC_ACTIVITY_EVENTS = new Set(
  Object.entries(EVENT_CLASSIFICATION)
    .filter(([evt, cls]) => cls === 'activity' && evt.startsWith('CodingAgent'))
    .map(([evt]) => evt)
);

export type CodingAgentWaitingInfo = {
  proposed: boolean;
  isExternalRepo: boolean;
  requiresRestart: boolean;
};

export function threadChangeWork(meta: ThreadMeta): ChangeWork {
  return changeWork(meta.codingAgentChangeState.kind, meta.liveEventWaitCount > 0);
}

export function getCodingAgentWaitingInfo(meta: ThreadMeta): CodingAgentWaitingInfo | null {
  const change = meta.codingAgentChangeState;
  if (meta.channel !== 'claude_code') return null;
  if (change.kind !== 'proposed') return null;
  if (meta.status === 'running') return null;
  return {
    proposed: true,
    isExternalRepo: meta.codingAgentIsExternalRepo,
    requiresRestart: change.requires_restart,
  };
}
