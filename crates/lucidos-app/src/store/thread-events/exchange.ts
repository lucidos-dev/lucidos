import { MODELS, REASONING_LEVELS } from '../models';
import { chatModels } from '../store';
import { ENGINE_LABEL, RESPONSE_CANCELED_SUMMARY, continuationStartedSummary, responseAbortedSummary } from './thread-event-types';
import { CC_ACTIVITY_EVENTS } from './thread-meta';
import type { ActorMode, SequencedEvent, StoredEvent, ThreadEvent, ThreadInitiator } from './thread-event-types';
import type { CodingAgent } from '../../generated/thread-event-wire';

/** A typed answer to a question card that the engine has not confirmed. It is
 *  either still sending, or its POST got no answer and it waits for a Retry
 *  (`unsentEventId` names the unsent message). */
export type TypedAnswer =
  | { state: 'sending'; text: string; image_hashes: string[] }
  | { state: 'unsent'; text: string; image_hashes: string[]; unsentEventId: string };

export type Exchange = {
  userEvent: StoredEvent;
  userSeq: number;
  steps: SequencedEvent[];
  /** True on divider exchanges (`userEvent.type === 'UserQuestionAsked'`)
   *  where the agent kept emitting progression events past the question
   *  without an answer (CC's parallel-tool-call race). Undefined on
   *  non-divider exchanges and on divider exchanges that have neither
   *  progression nor a matching answer yet. */
  questionOvertaken?: boolean;
  /** True on a CONTINUATION FRAGMENT: steps whose opening boundary is not
   *  loaded yet.
   *
   *  A long thread opens on its newest page, and that page routinely starts
   *  mid-turn. Without a fragment the fold drops every step ahead of the
   *  page's first boundary, and a page holding none draws nothing at all. The
   *  reader is then looking at a transcript too short to scroll, which is the
   *  only thing that fetches the page behind it.
   *
   *  It draws no initiator panel, because nothing loaded says who started the
   *  turn. Its `userEvent` is its own first step, so `exchangeKey` has a real
   *  id to key on. That id is never drawn twice: the panel the boundary would
   *  fill is the one this exchange omits.
   *
   *  Opened only while the thread reports older events unloaded. A thread
   *  served whole and missing a boundary is genuinely corrupt, and the
   *  transcript must keep saying so. */
  continuationFragment?: true;
  /** True once the fold handed this exchange's running turn to a LATER
   *  exchange — a `ChildThreadCompleted` card or a question / permission
   *  divider took over the request-id redirect, so every remaining event of
   *  the turn groups there instead. A `Thinking` marker still pending here can
   *  therefore never be resolved by its own exchange, and rendering finalizes
   *  it instead of shimmering forever (see `exchangeSteps` /
   *  `exchangeResponseEvents`). Cleared if the exchange takes the continuation
   *  back (a queued follow-up whose `PromptInjected` is absorbed, an
   *  answered divider re-anchored to its resolution point). Only Thinking
   *  markers are stale — a pending TOOL step can still be resolved by a result
   *  that re-routes back by tool id. */
  continuationMoved?: boolean;
  /** True when a running turn's continuation was handed to this exchange by a
   *  boundary that STARTED no turn of its own.
   *
   *  One case today: a caller's utterance landing mid-turn. The doer keeps
   *  working, and everything it emits from then on happened after those words,
   *  so it reads below them (ADR 0201). The words started nothing. The card
   *  still holds a turn, and `exchangeHoldsNoTurn` has to say so, or the
   *  status machinery steps over the card the work is in. */
  tookTheTurn?: boolean;
  /** The coding agent's `CodingAgentInputRead` for the message that opened
   *  this exchange. Drives the Sent / Read marker and dates the read. */
  inputRead?: StoredEvent;
  /** True while a message sent behind a running coding-agent turn waits for
   *  the agent to read it. It takes no steps meanwhile and renders in the
   *  bottom queue. See `docs/plans/2026-09-24-unread-coding-agent-messages-queue.md`. */
  awaitingRead?: true;
  /** On a question divider: the answer the user typed into the composer, not
   *  yet confirmed by the engine. Written by `foldedExchanges` on a clone. */
  typedAnswer?: TypedAnswer;
  /** True on the delivered copy of a message the engine held behind an open
   *  question or permission card (ADR 0256). Its header says it waited. */
  releasedFromHold?: true;
  /** Ids of this exchange's `MessageHeld` rows whose delivered copy is folded.
   *  That copy is now the message's one card, so the held row stops drawing.
   *  A release with no copy keeps its row, so a lost delivery stays visible.
   *  Written from outside, like `blockedStepSeqs`. */
  deliveredHeldIds?: Set<string>;
  /** `seq` of every tool call in this exchange that a permission card is
   *  holding, and of every one a decision refused. The renderer reads them at
   *  the `ToolCalled` / `CodingAgentToolCalled` case and emits `'blocked'` /
   *  `'denied'` in place of `'pending'` (see `StepOutcome`).
   *
   *  They live on the Exchange because the request is an exchange STARTER. The
   *  call it gates therefore sits in the PREVIOUS exchange, and nothing in this
   *  one's own steps could say so. The fold writes them, being the only layer
   *  holding both sides: `toolCallOwners` maps the CC / Codex `tool_use_id` to
   *  the owning exchange, and `gatedCalls` carries the pair forward so the
   *  resolution finds the same row. Undefined until a card gates something, so
   *  a thread that never sees one allocates nothing. */
  blockedStepSeqs?: Set<number>;
  deniedStepSeqs?: Set<number>;
  /** What the talker's LIVE row in this exchange says right now.
   *
   *  A step whose TEXT moves under a stable identity, which is the one shape
   *  the memo's step fingerprint cannot see. The row keeps its seq and the
   *  step count does not change. Without this field every word after the first
   *  render compares equal, and the bubble stops mid-sentence. The caller's own
   *  bubble is the other such row, and `userBubbleText` covers that one.
   *
   *  Written by `withLiveCallRows` on the clone it already makes, so reading it
   *  costs the memo nothing. Undefined on every exchange holding no live
   *  reply, which is all of them outside a call. */
  liveReplyText?: string;
  /** Mutation counter for the incremental grouping cache. The cached fold
   *  mutates Exchange objects IN PLACE on later appends (steps push,
   *  questionOvertaken flip, absorb re-anchor), so a memo comparing
   *  `prev.exchange` against `next.exchange` would compare the same mutated
   *  object with itself and never detect a change. Render sites capture this
   *  number as a primitive prop at render time (`revision={ex.revision ?? 0}`)
   *  and the memo compares the captured values. Bumped by
   *  `groupIntoExchangesCached` for every exchange an appended event touched;
   *  undefined on exchanges from a from-scratch fold (fresh objects — the
   *  captured 0 plus the field-level fingerprint covers those). */
  revision?: number;
};

/** Stable render key for an exchange — survives the optimistic→persisted swap.
 *  An optimistic pending message renders as a synthetic exchange at a
 *  MAX_SAFE_INTEGER `userSeq`; the persisted event that replaces it carries its
 *  real (much smaller) DB seq. Keying the rendered `<ChatExchange>` by `userSeq`
 *  would therefore change the key on the swap, remounting the DOM node — which
 *  churns the auto-scroll observers and makes the just-sent follow-up visibly
 *  disappear, then reappear once a later event re-snaps to the bottom. The
 *  client `event_id` round-trips as the events-table primary key (see
 *  `EventMeta.event_id`), so `userEvent._eventId` is identical across the swap
 *  and is the right identity to key on. Fall back to a `seq:`-prefixed seq for
 *  events without an id (legacy rows, synthetic boundaries); the prefix keeps a
 *  fallback key from ever colliding with an `_eventId` value. */
export function exchangeKey(exchange: Exchange): string {
  const id = exchange.userEvent._eventId;
  return id ? `id:${id}` : `seq:${exchange.userSeq}`;
}

/** The narrowed `UserQuestionAnswered` variant — exposed so call sites that
 *  walk an Exchange's steps can read the question's resolution (answer + actor)
 *  without redeclaring the shape. */
export type AnsweredQuestion = Extract<StoredEvent, { type: 'UserQuestionAnswered' }>;

/** The narrowed `CodingAgentPermissionResolved` variant — same purpose as
 *  `AnsweredQuestion`, for permission-prompt resolutions. */
export type ResolvedPermission = Extract<StoredEvent, { type: 'CodingAgentPermissionResolved' }>;

/** The narrowed `CommandPermissionResolved` variant (ADR 0002) — the chat
 *  command-guard counterpart of `ResolvedPermission`. */
export type ResolvedCommandPermission = Extract<StoredEvent, { type: 'CommandPermissionResolved' }>;

/** The narrowed `McpPermissionResolved` variant — the chat MCP counterpart of
 *  `ResolvedPermission`. */
export type ResolvedMcpPermission = Extract<StoredEvent, { type: 'McpPermissionResolved' }>;

/** Find the matching `UserQuestionAnswered` step in a divider exchange.
 *  Returns the typed event (with `answer` narrowed and the optional `actor`
 *  stamped by `EventMeta`) or undefined when the question is still pending. */
export function findQuestionAnswer(exchange: Exchange, toolUseId: string): AnsweredQuestion | undefined {
  for (const { event } of exchange.steps) {
    if (event.type === 'UserQuestionAnswered' && event.tool_use_id === toolUseId) return event;
  }
  return undefined;
}

/** Find the matching `CodingAgentPermissionResolved` step in a permission
 *  divider exchange. Returns the typed event or undefined when the request
 *  is still pending. */
export function findPermissionResolution(exchange: Exchange, requestId: string): ResolvedPermission | undefined {
  for (const { event } of exchange.steps) {
    if (event.type === 'CodingAgentPermissionResolved' && event.request_id === requestId) return event;
  }
  return undefined;
}

/** Find the matching `CommandPermissionResolved` step in a command-guard
 *  permission divider exchange (ADR 0002). Returns the typed event or
 *  undefined when the request is still pending. */
export function findCommandPermissionResolution(
  exchange: Exchange,
  requestId: string,
): ResolvedCommandPermission | undefined {
  for (const { event } of exchange.steps) {
    if (event.type === 'CommandPermissionResolved' && event.request_id === requestId) return event;
  }
  return undefined;
}

/** Find the matching `McpPermissionResolved` step in a chat MCP permission
 *  divider exchange. Returns the typed event or undefined when still pending. */
export function findMcpPermissionResolution(
  exchange: Exchange,
  requestId: string,
): ResolvedMcpPermission | undefined {
  for (const { event } of exchange.steps) {
    if (event.type === 'McpPermissionResolved' && event.request_id === requestId) return event;
  }
  return undefined;
}

// ---------------------------------------------------------------------------
// Exchange-level derived data — standalone functions on Exchange.
// ---------------------------------------------------------------------------

/** Which boundaries the reader owns, and so draw the right-aligned bubble.
 *
 *  Two events, one act. A caller's utterance is a `SpokenMessageReceived` when
 *  the talker answered it alone and a `MessageReceived` when it delegated. The
 *  reader said the same thing either way, so both read the same way. The
 *  transcript draws these as bubbles, and its find bar searches them. */
export function isUserBubbleEvent(userEvent: { type: string }): boolean {
  return userEvent.type === 'MessageReceived' || userEvent.type === 'SpokenMessageReceived';
}

/**
 * Does this turn draw the panel naming who started it?
 *
 * Everything does, except a resume (see `isResumeTurn`), a successor
 * session's opening (see `isSuccessorSessionOpening`) and a continuation
 * fragment. A fragment's boundary is older than the page this client holds, so
 * nothing loaded can name it. Its `userEvent` is only its own first step,
 * standing in for a key. Drawing that as an initiator would put a tool call
 * where the reader expects a person.
 *
 * The rows themselves still draw. The head of the turn arrives with the page
 * behind it, and the fold then merges the fragment into the real turn. See
 * `Exchange.continuationFragment`. Find in the transcript counts a user's
 * message only where this draws it.
 */
export function drawsInitiatorPanel(exchange: Exchange): boolean {
  return exchange.continuationFragment !== true && !isResumeTurn(exchange) && !isSuccessorSessionOpening(exchange);
}

/** The engine's message that opened a successor session. Its session card
 *  (`HandoffSessionRow`) already folds it, so a panel would be a second copy.
 *  Without that card, as when the spawn failed, the panel is the only place
 *  the message shows. */
function isSuccessorSessionOpening({ userEvent, steps }: Exchange): boolean {
  return userEvent.type === 'MessageReceived'
    && userEvent.origin?.kind === 'engine'
    && userEvent.origin.reason.kind === 'successor_session'
    && steps.some(({ event }) => event.type === 'SessionStarted' && event.continuity?.kind === 'handoff');
}

/** A resume opens no header of its own. It is a card at the top of the reply it
 *  resumed (`ResumeCard`), so the response panel draws even when that reply is
 *  still empty. */
export function isResumeTurn(exchange: Exchange): boolean {
  return exchange.userEvent.type === 'ContinuationStarted';
}

/** Derive the user message text from an exchange. */
export function exchangeUserMessage(exchange: Exchange): string {
  const ev = exchange.userEvent;
  if (ev.type === 'TriggerStarted') {
    return ev.prompt || ev.trigger_name || '';
  }
  if (ev.type === 'ContinuationStarted') {
    // One event covers three triggers; `continuationStartedSummary` reads
    // `reason` first so an `auto_recovery_after_hang` resume (a local hang or
    // stray signal-kill — NOT a restart) isn't mislabeled "Resumed after
    // engine restart", then falls back to actor (human = clicked Continue).
    return continuationStartedSummary(ev.reason, ev.actor);
  }
  if (ev.type === 'ResponseAborted') {
    return responseAbortedSummary(ev.actor, ev.cause);
  }
  if (ev.type === 'ResponseCanceled') {
    return RESPONSE_CANCELED_SUMMARY;
  }
  if (ev.type === 'MissingHardeningDetected') {
    return `${ENGINE_LABEL} — Hardening`;
  }
  if (ev.type === 'MergeConflictDetected') {
    const files = ev.files ?? [];
    const suffix = files.length > 0 ? ` (${files.length} file${files.length === 1 ? '' : 's'})` : '';
    return `${ENGINE_LABEL} — Merging changes from main${suffix}`;
  }
  if (isChangeLifecycleEvent(ev)) return '';
  if ('text' in ev) return (ev as { text: string }).text;
  return '';
}

/** Change lifecycle event types — render as terminal initiator-only panels. */
export type ChangeLifecycleType =
  | 'ChangeApplied' | 'ChangeDiscarded' | 'ChangeReverted' | 'ChangeApplyFailed';

export type ChangeLifecycleEvent = Extract<ThreadEvent, { type: ChangeLifecycleType }>;

const CHANGE_LIFECYCLE_TYPES: ReadonlySet<string> = new Set([
  'ChangeApplied', 'ChangeDiscarded', 'ChangeReverted', 'ChangeApplyFailed',
]);

export function isChangeLifecycleEvent(event: { type: string }): event is ChangeLifecycleEvent {
  return CHANGE_LIFECYCLE_TYPES.has(event.type);
}

/** Map an `ActorMode` to the UI's binary user-vs-system label.
 *  Undefined defaults to `'user'` (mirrors the engine's `default_mode_human`
 *  for old DB rows persisted before the `mode` field existed). */
export function modeToInitiator(mode: ActorMode | undefined): ThreadInitiator {
  return mode === 'agent' || mode === 'engine' ? 'system' : 'user';
}

/** Extract user-pasted image hashes from the exchange's MessageReceived event.
 *  Post-Phase-3b the event payload carries `user_image_hashes: string[]` only;
 *  the bytes live in the content-addressed blob store and are loaded by the
 *  renderer via `<img src="/api/v1/blobs/<hash>">`. */
export function exchangeUserImageHashes(exchange: Exchange): string[] {
  if (exchange.userEvent.type !== 'MessageReceived') return [];
  const raw = (exchange.userEvent as { user_image_hashes?: unknown }).user_image_hashes;
  if (!Array.isArray(raw)) return [];
  return raw.filter((h): h is string => typeof h === 'string');
}

/** Extract a field from the response completion event, else from the newest step
 *  `midTurn` reads it from. Walks steps backward, skipping terminal events that
 *  omit the field (recovery paths emit ResponseAborted with model=null). A
 *  coding-agent session reads its steps mid-turn. Chat sessions fall back to the
 *  request metadata stamped on the exchange's STARTER event so the route popover
 *  shows model/effort while the response is still streaming.
 *
 *  Both starter kinds carry that metadata and both are read here, mirroring
 *  `threadModelSelections`' newest-pair lookup and the backend's
 *  starter kinds in `last_thread_chat_settings`: a chat turn starts with
 *  `MessageReceived`, a trigger fire starts with `TriggerStarted` and emits no
 *  `MessageReceived` at all. Reading only the former left a firing trigger's
 *  Executor section with no Model and no Effort row for the whole run, even
 *  though the fire had resolved both. */
type ResponseField = 'model' | 'reasoning_effort';
function extractResponseField(
  exchange: Exchange,
  field: ResponseField,
  midTurn: (event: StoredEvent) => string | undefined,
): string | undefined {
  let ccFallback: string | undefined;
  for (let i = exchange.steps.length - 1; i >= 0; i--) {
    const event = exchange.steps[i].event;
    if (event.type === 'ResponseGenerated' || event.type === 'ResponseCanceled' || event.type === 'ResponseAborted') {
      const v = event[field];
      if (v) return v;
    }
    ccFallback ??= midTurn(event);
  }
  if (ccFallback) return ccFallback;
  const starter = exchange.userEvent;
  if (starter.type === 'MessageReceived' || starter.type === 'TriggerStarted') {
    const v = starter[field];
    if (v) return v;
  }
  return undefined;
}

/** A `CodingAgentSettingsChanged` the engine wrote at spawn, never a user's
 *  pick. A pick carries an `actor`: it is a selection for the next turn, and
 *  it lands among the steps of the exchange before it (ADR 0542). */
function spawnSettings(event: StoredEvent): Extract<StoredEvent, { type: 'CodingAgentSettingsChanged' }> | undefined {
  return event.type === 'CodingAgentSettingsChanged' && !(event as { actor?: unknown }).actor ? event : undefined;
}

/** The model the exchange's calls ran on. Mid-turn that is the settings the
 *  engine wrote at spawn, else the model stamped on the turn's starter. */
export function exchangeResponseModel(exchange: Exchange): string | undefined {
  return extractResponseField(exchange, 'model', (event) => spawnSettings(event)?.model || undefined);
}

/** The effort the exchange's calls ran at, its *effort in force*. Mid-turn that
 *  is the newest main-session capture, or the settings the engine wrote at
 *  spawn. */
export function exchangeReasoningEffort(exchange: Exchange): string | undefined {
  return extractResponseField(exchange, 'reasoning_effort', (event) => {
    if (event.type === 'ContextCaptured') {
      return parentToolUseIdOf(event) ? undefined : event.reasoning_effort || undefined;
    }
    return spawnSettings(event)?.reasoning_effort || undefined;
  });
}

/** The backend each exchange's session ran on, index for index. A handoff may
 *  switch backend (ADR 0560). So each `SessionStarted` decides the turns up to
 *  the next one, and the newest in an exchange decides that exchange.
 *
 *  The loaded events are the thread's newest page, so turns above the first
 *  loaded session ran in the session before it. A resume or a fresh session
 *  keeps the backend, so they take that session's own. Above a handoff, or
 *  with no session loaded at all, they take `threadBackend`, the newest
 *  session's. That holds until the older page loads. */
export function sessionCodingAgents(exchanges: Exchange[], threadBackend: CodingAgent): CodingAgent[] {
  const backends: CodingAgent[] = [];
  let current: CodingAgent | undefined;
  for (const exchange of exchanges) {
    for (const { event } of exchange.steps) {
      if (event.type !== 'SessionStarted') continue;
      // A row from before the field existed is Claude Code, the wire default.
      const backend = event.coding_agent ?? 'claude-code';
      if (current === undefined) {
        backends.fill(event.continuity?.kind === 'handoff' ? threadBackend : backend);
      }
      current = backend;
    }
    backends.push(current ?? threadBackend);
  }
  return backends;
}

// Static fallback labels: the `MODELS` fallback list plus legacy model strings
// and Claude Code session short aliases (`CodingAgentSettingsChanged.model`
// carries these verbatim, so without an explicit label the popover renders the
// bare alias, e.g. `opus[1m]`). The loaded registry takes precedence in
// `displayModelName` so user-added models render their chosen label.
const STATIC_MODEL_LABELS: Record<string, string> = Object.fromEntries([
  ...MODELS.map(m => [m.value, m.label]),
  // Models pruned from the picker (disabled in the registry) but still present
  // in historical exchanges — keep their labels so old threads don't render a
  // bare id.
  ['claude-opus-4-8@default', 'Opus 4.8'],
  ['claude-opus-4-8@default[1m]', 'Opus 4.8 (1M)'],
  ['claude-opus-4-7', 'Opus 4.7'],
  ['claude-opus-4-7[1m]', 'Opus 4.7 (1M)'],
  ['claude-sonnet-4-6', 'Sonnet 4.6'],
  ['claude-sonnet-4-6[1m]', 'Sonnet 4.6 (1M)'],
  ['claude-opus-4-6', 'Opus 4.6'],
  ['claude-opus-4-6[1m]', 'Opus 4.6 (1M)'],
  ['claude-opus-4-5@20251101', 'Opus 4.5'],
  ['gpt-5.5', 'GPT-5.5'],
  ['gpt-5.4', 'GPT-5.4'],
  ['gpt-5.3-codex', 'GPT-5.3 Codex'],
  ['gpt-5.2-codex', 'GPT-5.2 Codex'],
  ['gpt-5.3-codex-spark', 'Codex Spark'],
  ['claude-opus-4-1', 'Opus 4.1'],
  ['claude-opus-4-8[1m]', 'Opus 4.8 (1M)'],
  ['claude-opus-4-8', 'Opus 4.8'],
  ['claude-fable-5', 'Fable 5'],
  ['claude-fable-5[1m]', 'Fable 5 (1M)'],
  ['claude-opus-5', 'Opus 5'],
  ['claude-opus-5[1m]', 'Opus 5 (1M)'],
  ['claude-sonnet-5', 'Sonnet 5'],
  ['claude-sonnet-5[1m]', 'Sonnet 5 (1M)'],
  ['claude-fable-5-1[1m]', 'Fable 5.1 (1M)'],
  ['claude-opus-5-5[1m]', 'Opus 5.5 (1M)'],
  ['claude-sonnet-5-5[1m]', 'Sonnet 5.5 (1M)'],
  ['claude-haiku-4-5', 'Haiku 4.5'],
  ['gemini-3-flash-preview', 'Gemini 3 Flash'],
  ['gpt-5.6-sol', 'GPT-5.6 Sol'],
  ['gpt-5.6-terra', 'GPT-5.6 Terra'],
  ['gpt-5.6-luna', 'GPT-5.6 Luna'],
  ['gpt-5.5-pro', 'GPT-5.5 Pro'],
  ['gpt-5.4-mini', 'GPT-5.4 mini'],
  // The `@default` spellings the chat registry used to carry. They still reach
  // the transcript from two places the re-spell deliberately left alone: the
  // Claude Code picker, whose own vocabulary still says `claude-opus-5@default`,
  // and the cost ledger, which records what a past request literally named.
  ['claude-opus-5@default', 'Opus 5'],
  ['claude-opus-5@default[1m]', 'Opus 5 (1M)'],
  ['claude-haiku-4-5-20251001', 'Haiku 4.5'],
  ['claude-haiku-4-5@20251001', 'Haiku 4.5'],
  // Mirrors the picker rows, deliberately version-free: `opus` and `sonnet`
  // are CC's own always-latest aliases, so a stored version says nothing
  // about WHICH generation ran. Naming one goes stale the moment Anthropic
  // repoints the alias, which is how 'Opus 4.6' and 'Sonnet 4.6' both went
  // stale before.
  ['opus', 'Opus (latest)'],
  ['opus[1m]', 'Opus (latest, 1M)'],
  ['sonnet', 'Sonnet (latest)'],
  // `sonnet[1m]` was dropped from the CC picker (it selects nothing different
  // now that `sonnet` resolves to Sonnet 5 with a native 1M window). Older
  // threads still carry it, so keep the label they were pinned under.
  ['sonnet[1m]', 'Sonnet 4.6 (1M)'],
  ['haiku', 'Haiku 4.5'],
]);

export function displayModelName(modelId: string): string {
  const loaded = chatModels.value;
  if (loaded.status === 'loaded') {
    const m = loaded.data.find(x => x.id === modelId);
    if (m) return m.label;
  }
  return STATIC_MODEL_LABELS[modelId] ?? modelId;
}

const EFFORT_LABELS: Record<string, string> = Object.fromEntries(
  REASONING_LEVELS.map(l => [l.value, l.label]),
);

export function displayReasoningEffort(effort: string): string {
  return EFFORT_LABELS[effort] ?? effort;
}

/** Derive the user timestamp from an exchange. */
export function exchangeTimestamp(exchange: Exchange): string {
  return exchange.userEvent.created
    || exchange.userEvent._displayCreated
    || new Date().toISOString();
}

/** Steps that record something ABOUT a turn rather than part of it. None draws
 *  a row of its own, and each can land arbitrarily later than the work: an
 *  archive whenever the user gets round to it, a session end at the next
 *  engine shutdown, a form answered hours after the agent asked. Skipping them
 *  costs nothing, since the scan below steps over no row on its way past.
 *
 *  They stay steps. `exchangeStatus` reads `SessionEnded` for the shutdown and
 *  session-end verdicts, and dropping either from the walk would change
 *  grouping for every consumer to fix one header. */
const NON_RESPONSE_STEP_TYPES: ReadonlySet<string> = new Set([
  'ThreadArchived',
  'SessionEnded',
  'FormRequestResolved',
]);

/** Derive the response timestamp: when the turn's last visible step landed.
 *  Returns undefined if there are no such steps (no response yet).
 *
 *  Skipping `NON_RESPONSE_STEP_TYPES` is what keeps the header honest. Read one
 *  and the panel dates the turn by something outside it, so a quick reply
 *  archived hours later reports hours of work. */
export function exchangeResponseTimestamp(exchange: Exchange): string | undefined {
  for (let i = exchange.steps.length - 1; i >= 0; i--) {
    const { event } = exchange.steps[i];
    if (NON_RESPONSE_STEP_TYPES.has(event.type)) continue;
    if (event.created) return event.created;
  }
  return undefined;
}

/** Check if the exchange has actual CC content (tools/text, not just SessionStarted). */
export function exchangeHasCCContent(exchange: Exchange): boolean {
  return exchange.steps.some(({ event }) => CC_ACTIVITY_EVENTS.has(event.type));
}

/** Read `parent_tool_use_id` from a coding-agent step or capture payload: the
 *  `Agent` call whose sub-agent produced it. `undefined` for the session's own. */
export function parentToolUseIdOf(event: { type: string }): string | undefined {
  return (event as { parent_tool_use_id?: string }).parent_tool_use_id || undefined;
}

/** Read `api_call_id` from a coding-agent step or capture payload: the API
 *  call that produced it. `undefined` from Codex and on older rows. */
export function apiCallIdOf(event: { type: string }): string | undefined {
  return (event as { api_call_id?: string }).api_call_id || undefined;
}

/** Streamed text that is the agent's own reply. A sub-agent's narration rides
 *  the same event, tagged with its `Agent` call, and is never the reply. */
export function isAgentReplyText(event: { type: string }): boolean {
  return event.type === 'TextStreamed'
    || (event.type === 'CodingAgentTextStreamed' && !parentToolUseIdOf(event));
}

/** Build the response text by concatenating the agent's reply text, then the
 *  quiet reply no chunk streamed. */
export function exchangeResponseText(exchange: Exchange): string {
  let text = '';
  for (const { event } of exchange.steps) {
    if (isAgentReplyText(event)) text += (event as { text: string }).text;
  }
  const quietReply = exchangeQuietReply(exchange);
  if (quietReply === null) return text;
  return text.trim() ? `${text}\n\n${quietReply}` : quietReply;
}

/** If this exchange's terminator was the chat agent's per-turn cap, return the
 *  message body (the prose after "[ENGINE-LIMIT] "). Otherwise empty string.
 *
 *  The cap is emitted as `ResponseGenerated { text: "[ENGINE-LIMIT] …" }` by
 *  emit_iteration_cap_response_generated in agentic_loop.rs — with NO preceding
 *  TextStreamed, so the text never lands in exchangeResponseText. The UI needs
 *  this side channel to render the "cap reached" banner; without it the user
 *  just sees the agent silently stop. */
const ENGINE_LIMIT_PREFIX = '[ENGINE-LIMIT]';
export function exchangeEngineLimitDetail(exchange: Exchange): string {
  for (let i = exchange.steps.length - 1; i >= 0; i--) {
    const event = exchange.steps[i].event;
    if (event.type !== 'ResponseGenerated') continue;
    const text = event.text;
    if (text && text.startsWith(ENGINE_LIMIT_PREFIX)) {
      return text.slice(ENGINE_LIMIT_PREFIX.length).trim();
    }
    return '';
  }
  return '';
}

/** The *quiet reply* a quiet pick ended this exchange on, or null. The engine
 *  emits it as the `ResponseGenerated` text, and no chunk streams it. Null too
 *  when the agent streamed that same text after the pick, or for the cap. */
export function exchangeQuietReply(exchange: Exchange): string | null {
  let afterQuietPick = false;
  let streamed = '';
  for (const { event } of exchange.steps) {
    if (event.type === 'UserQuestionAnswered') {
      afterQuietPick = event.quiet === true;
      streamed = '';
    } else if (!afterQuietPick) {
      continue;
    } else if (isAgentReplyText(event)) {
      streamed += (event as { text: string }).text;
    } else if (event.type === 'ResponseGenerated') {
      const text = event.text ?? '';
      const drawn = text.trim() && text.trim() !== streamed.trim() && !text.startsWith(ENGINE_LIMIT_PREFIX);
      return drawn ? text : null;
    }
  }
  return null;
}
