import { EVENT_CLASSIFICATION, LAST_ACTIVITY_EVENTS } from '../../generated/thread-lifecycle';
import type { EventChannel, ThreadStatus } from '../../generated/thread-lifecycle';
import type { StoredEvent, ThreadInitiator, ThreadSection, TodoItem } from './thread-event-types';

/** Post-event projection snapshot carried on persisted SSE thread events
 *  (`data.aggregate`) and on `fetchThreadEvents` HTTP responses
 *  (`currentAggregate`). Mirrors the backend `ThreadAggregate` struct.
 *  Compose fields are intentionally excluded — they have their own
 *  broadcast cadence and including them would clobber local typing.
 *  Frontend overlays this onto `thread.meta` so it never has to derive
 *  thread state from event-type lookups (SECTION_TRANSITIONS / STATUS_TRANSITIONS). */
export type ThreadAggregate = {
  threadId: string;
  title: string;
  channel: string;
  initiator: ThreadInitiator;
  createdAt: string;
  lastActivity: string;
  /** When the user last drove this thread forward — the drawer sort key. The
   *  backend always sends it; optional only so test fixtures needn't set a
   *  recency they don't assert on (consumers fall back to `lastActivity`). */
  lastUserAction?: string;
  /** When the agent (or trigger) last did something — the tooltip's Agent line.
   *  Optional for the same reason as `lastUserAction`. */
  lastAgentAction?: string;
  messageCount: number;
  section: ThreadSection;
  status: ThreadStatus;
  activeChildrenCount: number;
  totalChildrenCount: number;
  /** Count of descendants (transitive) currently blocking this thread's
   *  archive. Maintained by EventBus on
   *  `thread_summaries.blocking_descendant_count`. Consumed by
   *  `resolveActions` via `count > 0`. */
  blockingDescendantCount: number;
  /** Count of descendants (transitive) currently in a state that needs user
   *  attention (WaitingForUserAnswer, or an in-workspace CC thread with
   *  pending changes). Strict subset of `blockingDescendantCount` — drops
   *  the Running case. Consumed by `displaySection` via `count > 0` to
   *  bubble the parent to REVIEW even when sibling descendants are still
   *  running. */
  attentionDescendantCount: number;
  /** Whether the CC branch has any diff against main on disk — pure git
   *  truth. Backs the WaitingBanner Diff button. Independent of the proposal
   *  lifecycle: a thread can have a diff mid-session before CC has formally
   *  proposed (and stays true between proposal and Apply / Discard). */
  codingAgentHasDiff: boolean;
  /** CC's formal "ready for review" — set by `ChangeProposed`, cleared on
   *  Apply / Discard / Archive. Backs the Apply / Discard buttons. */
  codingAgentProposed: boolean;
  /** Whether the proposed change requires an engine restart. Only meaningful
   *  when `codingAgentProposed` is true. */
  codingAgentRequiresRestart: boolean;
  /** Whether the Claude Code session is bound to an external repo. External repos
   *  can't be Applied via the engine merge flow — the WaitingBanner shows
   *  Done / Archive instead. */
  codingAgentIsExternalRepo: boolean;
  /** Whether a merge conflict is being resolved. */
  codingAgentApplying: boolean;
  isSaved: boolean;
  hasResponse: boolean;
  lastRevivedAt: string | null;
  parentThreadId: string | null;
  parentThreadTitle: string | null;
  triggerId?: string;
  triggerName?: string;
  ccRepoId?: string;
  ccRepoName?: string;
  /** Coding-agent thread flavor — drives app-specific affordances (WIP preview
   *  button, app-icon branch chip). Absent for non-CC threads and legacy CC
   *  rows; consumers default absence → 'lucidos'. */
  codingAgentKind?: 'lucidos' | 'app' | 'external';
  /** Canonical folder the coding agent operates on (`<ws>/data/apps/<id>/`
   *  for app threads). Absent for non-CC threads and legacy rows. */
  codingAgentFolder?: string;
  /** Which backend drives this thread — 'claude-code' | 'codex'. Absent for
   *  non-CC threads and legacy rows (consumers default to 'claude-code'). */
  codingAgent?: 'claude-code' | 'codex';
  state: ThreadComposeState;
};

/** Apply an aggregate snapshot to a thread's meta. Used by live SSE (per-event
 *  aggregate) and historical replay (fetchThreadEvents.currentAggregate).
 *  Nullable fields propagate cleared values; trigger/repo fields are omitted
 *  by the backend when not applicable, so absence preserves prior values.
 *
 *  Returns `true` when any shape-relevant field actually changed value. The
 *  `updatedAt` / `messageCount` ticks are intentionally excluded from the
 *  changed signal — they move on every streaming event and would defeat the
 *  fan-out gate in `thread-sync.ts`. ThreadDrawer's "X ago" stays approximate
 *  during a stream and refreshes on the next shape change (status flip etc.). */
export function applyAggregateToMeta(meta: ThreadMeta, agg: ThreadAggregate): boolean {
  let changed = false;
  if (meta.section !== agg.section) { meta.section = agg.section; changed = true; }
  if (meta.status !== agg.status) { meta.status = agg.status; changed = true; }
  if (meta.activeChildrenCount !== agg.activeChildrenCount) { meta.activeChildrenCount = agg.activeChildrenCount; changed = true; }
  if (meta.totalChildrenCount !== agg.totalChildrenCount) { meta.totalChildrenCount = agg.totalChildrenCount; changed = true; }
  if (meta.blockingDescendantCount !== agg.blockingDescendantCount) { meta.blockingDescendantCount = agg.blockingDescendantCount; changed = true; }
  if (meta.attentionDescendantCount !== agg.attentionDescendantCount) { meta.attentionDescendantCount = agg.attentionDescendantCount; changed = true; }
  if (meta.codingAgentHasDiff !== agg.codingAgentHasDiff) { meta.codingAgentHasDiff = agg.codingAgentHasDiff; changed = true; }
  if (meta.codingAgentProposed !== agg.codingAgentProposed) { meta.codingAgentProposed = agg.codingAgentProposed; changed = true; }
  if (meta.codingAgentRequiresRestart !== agg.codingAgentRequiresRestart) { meta.codingAgentRequiresRestart = agg.codingAgentRequiresRestart; changed = true; }
  if (meta.codingAgentIsExternalRepo !== agg.codingAgentIsExternalRepo) { meta.codingAgentIsExternalRepo = agg.codingAgentIsExternalRepo; changed = true; }
  if (meta.codingAgentApplying !== agg.codingAgentApplying) { meta.codingAgentApplying = agg.codingAgentApplying; changed = true; }
  if (meta.saved !== agg.isSaved) { meta.saved = agg.isSaved; changed = true; }
  // updatedAt / messageCount: overlay unconditionally, do NOT mark changed
  meta.messageCount = agg.messageCount;
  meta.updatedAt = agg.lastActivity;
  // lastUserAction is the drawer SORT key — mark changed so a user action
  // re-sorts the list immediately. lastAgentAction is tooltip-only, so overlay
  // it like updatedAt (no `changed`) — it moves on every agent event and would
  // otherwise defeat the fan-out gate in thread-sync.ts. Only overlay when the
  // aggregate carries them (it always does in prod) so a field-less test
  // aggregate can't blank a previously-set value.
  if (agg.lastUserAction !== undefined && meta.lastUserAction !== agg.lastUserAction) { meta.lastUserAction = agg.lastUserAction; changed = true; }
  if (agg.lastAgentAction !== undefined) meta.lastAgentAction = agg.lastAgentAction;
  const nextLastRevived = agg.lastRevivedAt ?? '';
  if (meta.lastRevivedAt !== nextLastRevived) { meta.lastRevivedAt = nextLastRevived; changed = true; }
  if (meta.state !== agg.state) { meta.state = agg.state; changed = true; }
  const nextParentId = agg.parentThreadId ?? undefined;
  if (meta.parentThreadId !== nextParentId) { meta.parentThreadId = nextParentId; changed = true; }
  const nextParentTitle = agg.parentThreadTitle ?? undefined;
  if (meta.parentThreadTitle !== nextParentTitle) { meta.parentThreadTitle = nextParentTitle; changed = true; }
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
  return changed;
}

/** Placeholder shown in the drawer while a thread waits for its first
 *  LLM-generated title. Treated as "no title" by anything that displays it. */
export const PENDING_TITLE_PLACEHOLDER = '...';

export type ThreadMeta = {
  id: string;
  title: string;
  channel: EventChannel | 'error_unknown_channel';
  initiator: ThreadInitiator;
  saved: boolean;
  createdAt: string;
  updatedAt: string;
  /** When the user last drove this thread forward (message sent, question
   *  answered, permission resolved, change applied/discarded). The drawer SORTS
   *  by this — agent streaming/idle does NOT bump it — so background agent churn
   *  no longer reshuffles the list. Optional only for test ergonomics; every
   *  production path (API load, optimistic insert, aggregate overlay) sets it,
   *  and `recencyKey` falls back to `updatedAt` if it's somehow absent. */
  lastUserAction?: string;
  /** When the agent (or trigger) last did something — streaming, a terminal
   *  response, an idle, a trigger fire/complete, or asking the user. Drives the
   *  thread-row tooltip's "Agent ·" line, distinct from `lastUserAction` so the
   *  tooltip stays accurate even right after the user acts. */
  lastAgentAction?: string;
  /** Thread status computed by the backend: 'idle', 'running', or 'waiting'. */
  status: ThreadStatus;
  /** Server-computed exchange count (MESSAGE_COUNT_EVENTS in thread_lifecycle.rs). */
  messageCount: number;
  /** Section from backend DB projection — used as initial section before events load. */
  section: ThreadSection;
  /** Number of active child threads (non-zero means parent is "in progress"). */
  activeChildrenCount: number;
  /** Total number of child threads (active + finished). */
  totalChildrenCount: number;
  /** Count of descendants (transitive) currently blocking this thread's
   *  archive. Consumed by `resolveActions` via `count > 0`. */
  blockingDescendantCount: number;
  /** Count of descendants (transitive) currently in a state that needs user
   *  attention (WaitingForUserAnswer, or an in-workspace CC thread with
   *  pending changes). Strict subset of `blockingDescendantCount` — drops the
   *  Running case. Consumed by `getThreadDisplaySection` via `count > 0`. */
  attentionDescendantCount: number;
  /** Whether the CC branch has any diff against main on disk — pure git
   *  truth. Backs the WaitingBanner Diff button. Independent of the proposal
   *  lifecycle: a thread can have a diff mid-session before CC has formally
   *  proposed (and stays true between proposal and Apply / Discard). */
  codingAgentHasDiff: boolean;
  /** CC's formal "ready for review" — set by `ChangeProposed`, cleared on
   *  Apply / Discard / Archive. Backs the Apply / Discard buttons. */
  codingAgentProposed: boolean;
  /** Whether the proposed change requires an engine restart. Only meaningful
   *  when `codingAgentProposed` is true. */
  codingAgentRequiresRestart: boolean;
  /** Whether the Claude Code session is bound to an external repo — drives the
   *  WaitingBanner Done / Archive vs Apply choice. */
  codingAgentIsExternalRepo: boolean;
  /** Whether a merge conflict is being resolved. */
  codingAgentApplying: boolean;
  /** When the thread last entered 'running' state (for IN PROGRESS sort order). */
  lastRevivedAt: string;
  /** Set when mode != 'human' on the initial MessageReceived. */
  parentThreadId?: string;
  parentThreadTitle?: string;
  /** Trigger that fired this thread (only for `channel === 'trigger'`). */
  triggerId?: string;
  /** Trigger name at fire-time (snapshot — falls back when the trigger is renamed/deleted). */
  triggerName?: string;
  /** Repository the Claude Code session bound to (only for `channel === 'claude_code'`). */
  repoId?: string;
  /** Current repo name from the registry — undefined when the repo was deleted. */
  repoName?: string;
  /** Coding-agent thread flavor — 'lucidos' | 'app' | 'external'. Drives
   *  app-specific affordances (WIP preview button, app-icon branch chip).
   *  Absent for non-CC threads and legacy rows (consumers default to
   *  'lucidos'). */
  codingAgentKind?: 'lucidos' | 'app' | 'external';
  /** Canonical folder the coding agent operates on. For app threads,
   *  `<ws>/data/apps/<id>/` — the last path segment is the app id. */
  codingAgentFolder?: string;
  /** Which backend drives this thread — 'claude-code' | 'codex'. Bound at
   *  compose promotion (sendCompose) for new threads; loaded from the
   *  thread summary afterwards. Absent = legacy / 'claude-code'. */
  codingAgent?: 'claude-code' | 'codex';
  /** Compose state machine. Server is the source of truth; events flow via
   *  ThreadStarted, MessageReceived, ThreadDiscarded, ThreadArchived.
   *
   *  Draft text / images / mode pick live in the sibling `composeDrafts`
   *  signal (see `store/composeDrafts.ts`). They are NOT on ThreadMeta:
   *  per-keystroke draft writes would otherwise re-render every component
   *  subscribed to threadMap (most expensively ChatExchange, which calls
   *  marked.parse per render). */
  state: ThreadComposeState;
  /** Current *Todo list* snapshot — overwritten each time a `TodoListWritten`
   *  event arrives in `handleEvent`. `null` until the agent first writes one;
   *  `[]` is a valid "cleared" state. Projected here (rather than re-derived
   *  per render) so the prompt-bar indicator doesn't walk the events Map on
   *  every threadMap flush — see `TodoListIndicator`. */
  latestTodoList: TodoItem[] | null;
};

/** Compose state machine — mirrors the Rust `ThreadState` enum. The archive
 *  flag is intentionally NOT here; it lives on the separate `ThreadSection`
 *  ('inbox' | 'archived') maintained by the contract layer. An archived
 *  thread carries `state='active'` plus `archive_state='archived'`. */
export type ThreadComposeState = 'composing' | 'active' | 'discarded';
export type ComposeChannelMode = 'lucidos' | 'claude_code' | null;

export type ThreadState = {
  meta: ThreadMeta;
  events: Map<number, StoredEvent>;
  streamingBuffer: string;
  eventsLoaded: boolean;
  /** True when loadThreadEvents exhausted retries. The UI shows an error
   *  state instead of a spinner. On next resume, runResumeSync retries
   *  failed threads via loadThreadEvents (which resets this flag). */
  eventsLoadFailed: boolean;
  /** SSE events may arrive out of order during reconnect, so we track
   *  the max DB-loaded sequence separately to avoid skipping gap events. */
  lastDbSeq: number;
  /** Optimistic user messages shown before real SSE events arrive.
   *  Each entry is removed when its corresponding MessageReceived event arrives
   *  from SSE, matched by the client-generated event_id UUID. */
  pendingUserMessages: Array<{ text: string; eventId: string; created: string; image_hashes?: string[] }>;
};

/** Build a fresh `ThreadState` for optimistic / SSE-bootstrapped threads.
 *  All CC/changes flags default to false and counts to 0; callers override
 *  what they actually know. Centralised so adding a `ThreadMeta` field is a
 *  one-line change instead of a four-place audit.
 *
 *  Compose draft (text/images/mode) lives in the sibling `composeDrafts`
 *  signal — callers that bootstrap a `composing` thread (compose.ts,
 *  thread-sync.ts ThreadStarted skeleton) seed the draft entry separately
 *  via `setDraft`. Keeps this builder free of signal side effects. */
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
  /** Override compose state — defaults to 'active'. Set 'composing' for
   *  optimistic draft creation (compose.ts, ThreadStarted SSE). */
  state?: ThreadComposeState;
  /** Override status — defaults to 'running'. Composing rows want 'idle'. */
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
      messageCount: 0,
      section: 'archived',
      activeChildrenCount: 0,
      totalChildrenCount: 0,
      blockingDescendantCount: 0,
      attentionDescendantCount: 0,
      codingAgentHasDiff: false,
      codingAgentProposed: false,
      codingAgentRequiresRestart: false,
      codingAgentIsExternalRepo: false,
      codingAgentApplying: false,
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
    },
    events: new Map(),
    streamingBuffer: '',
    eventsLoaded: opts.eventsLoaded,
    eventsLoadFailed: false,
    lastDbSeq: 0,
    pendingUserMessages: opts.pendingUserMessages ?? [],
  };
}

/** Recency key for drawer ordering: when the USER last acted on the thread, so
 *  background agent churn (streaming, idle) doesn't reshuffle the list. Falls
 *  back to `updatedAt` (last activity) only if `lastUserAction` is absent — the
 *  backend always sends it, so the fallback just keeps older test fixtures and
 *  any pre-field skeleton sane. */
export const recencyKey = (t: ThreadState): string =>
  t.meta.lastUserAction || t.meta.updatedAt;

/** Sort threads by last user action descending (most recent first). */
export const byRecent = (a: ThreadState, b: ThreadState): number =>
  recencyKey(b).localeCompare(recencyKey(a));

/** Sort threads by creation time descending (newest created first). The drawer's
 *  Current section orders by this so the list stays stable: a thread holds its
 *  position regardless of agent churn or attention state — the attention/drafts
 *  filter icons surface those subsets instead of reshuffling the list. Falls
 *  back to `updatedAt` only for skeleton rows that haven't received a
 *  `createdAt` yet (the backend always sends one for real threads). */
export const byCreated = (a: ThreadState, b: ThreadState): number =>
  (b.meta.createdAt || b.meta.updatedAt).localeCompare(a.meta.createdAt || a.meta.updatedAt);

/** Threads whose compose state hides them from every drawer section: composing
 *  drafts live in the compose pane / Drafts surface, and discarded threads
 *  are tombstones. Any code that derives "what shows in a drawer section"
 *  (drawer rendering, the post-archive sibling picker, attention badges)
 *  must skip these so the count matches what the user can actually see. */
export const isExcludedFromSections = (t: ThreadState): boolean =>
  t.meta.state === 'composing' || t.meta.state === 'discarded';

/** Review-section sort tier for a thread under a given status — lower sorts
 *  higher. Three tiers:
 *    0 — WaitingForUserAnswer: a user question or permission request is
 *        blocking the agent. Most critical (nothing progresses until the user
 *        answers), so these float to the very top.
 *    1 — other CTA: codingAgentProposed (a change is ready to review) or Failed
 *        (the last response errored). The user should act, but no agent is
 *        stalled waiting on them.
 *    2 — no CTA: running, idle, etc.
 *  Tiers 0 and 1 together are "needs attention" (the count badges); tier 0 is
 *  the most-critical subset. The caller passes the status to consult — every
 *  caller (the drawer's family-aware sort via `computeFamilyKeys`, the attention
 *  view, and the post-archive focus picker) uses `effectiveThreadStatus`, which
 *  honors optimistic archiving + pending sends. */
export function reviewTier(t: ThreadState, status: ThreadStatus): 0 | 1 | 2 {
  if (status === 'waiting_for_user_answer') return 0;
  if (t.meta.codingAgentProposed || status === 'failed') return 1;
  return 2;
}

/** Whether this event type updates the thread's last_activity in the backend
 *  projection (event_bus.rs). Generated from thread_lifecycle.rs. */
export function updatesLastActivity(type: string): boolean {
  return LAST_ACTIVITY_EVENTS.has(type);
}

/** CC activity event types — tool calls, text streaming, and tool results.
 *  Used to detect active CC work after mid-session completion events.
 *  Derived from the generated thread lifecycle contract. */
export const CC_ACTIVITY_EVENTS = new Set(
  Object.entries(EVENT_CLASSIFICATION)
    .filter(([evt, cls]) => cls === 'activity' && evt.startsWith('CodingAgent'))
    .map(([evt]) => evt)
);

/** CC waiting info — sourced from backend thread_summaries projection. Used
 *  by the WaitingBanner to decide whether Apply / Discard appear and how
 *  they're rendered. `proposed` mirrors the existence of a `changes` row in
 *  the engine DB (both written in the `ChangeProposed` projection tx); use
 *  it as a union with the frontend `pendingChange` lookup so the banner
 *  doesn't flash between the two SSE broadcasts. */
export type CodingAgentWaitingInfo = {
  proposed: boolean;
  isExternalRepo: boolean;
  requiresRestart: boolean;
  applying: boolean;
};

/** Get CC waiting info from thread meta. Returns null for non-CC threads,
 *  threads without a pending proposal, and threads whose loop is mid-stream
 *  (status==='running' means a follow-up turn is in flight, so the user is
 *  not yet being asked to review). */
export function getCodingAgentWaitingInfo(meta: ThreadMeta): CodingAgentWaitingInfo | null {
  if (meta.channel !== 'claude_code') return null;
  if (!meta.codingAgentProposed) return null;
  if (meta.status === 'running') return null;
  return {
    proposed: meta.codingAgentProposed,
    isExternalRepo: meta.codingAgentIsExternalRepo,
    requiresRestart: meta.codingAgentRequiresRestart,
    applying: meta.codingAgentApplying,
  };
}
