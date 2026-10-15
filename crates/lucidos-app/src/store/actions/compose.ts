/**
 * Server-only compose state. No localStorage.
 *
 * Draft text / images / mode pick live in the sibling `composeDrafts` signal
 * (see `store/composeDrafts.ts`). `threadMap[id].meta.state` is the lifecycle
 * marker. The draft signal moves separately, so per-keystroke writes do not
 * ripple through every threadMap subscriber. Mutations go through:
 *   - updateCompose(id, patch)         — optimistic local + debounced PUT
 *   - startComposeIfNeeded(id, mode)   — POST /threads (idempotent)
 *   - discardCompose(id)               — DELETE /threads/:id, state→discarded
 *   - sendCompose(id, text, images, opts) — chat POST + state→active locally
 *   - sendFollowup(id, text, images, opts) — chat POST on already-active thread; clears draft
 *
 * Every report of what the engine stores goes through `store/composeSync.ts`.
 * It orders each by its *compose revision* and lays this device's pending
 * edits over it (ADR 0437).
 */

import { effect } from '@preact/signals';
import { threadMap, focusedThreadId, inputMode, showToast, removeToast, setFocusedThread, selectedScope, repositories, type Scope } from '../store';
import { loadedOr } from '../types';
import { generateUuid } from '../../utils/uuid';
import {
  composeSelections,
  pendingComposeSelection,
  getComposeSelectionOverride,
  patchComposeSelection,
  clearComposeSelection,
  seedComposeSelection,
  takePendingComposeSelection,
  resolveScope,
  resolveCodingAgent,
  resolveModel,
  resolveReasoningEffort,
  resolveProvider,
  resolveCcModel,
  resolveCcReasoningEffort,
  type ComposeSelectionOverride,
} from '../composeSelections';
import { scopeRepoName, type ComposeDestination } from '../composeDestination';
import { makeOptimisticThreadState, type ThreadMeta } from '../thread-events';
import { clearDraft, composeDrafts, draftIsEmpty, getDraft, patchDraft, setDraft, type ComposeDraft } from '../composeDrafts';
import { API, ApiError, ensureThreadStarted, putComposeOnThread, deleteThread, isTransientFetchError, type ComposePutResult } from '../../api/client';
import {
  acceptComposeReport,
  acknowledgeComposeEdits,
  composeEditSeq,
  forgetComposeSync,
  knownComposeEpoch,
  noteLocalComposeEdit,
  sameHashes,
  serverComposeOf,
  _resetComposeSyncForTesting,
  type ComposeReport,
} from '../composeSync';
import { errorDetail } from '../../utils/errorDetail';
import { deviceIdHeader } from '../../utils/deviceIdHeader';
import { createFailureCounter } from '../../utils/failureCounter';
import { appendMessagesToCompose, sendMessage } from './chat';
import { refreshRepositories } from './repositoriesLoader';
// Cycle-safe: `compose -> chat -> thread-loading -> compose` already exists, and
// this is a function declaration called at runtime, never at module init.
import { forgetThreadEventsFailures } from './thread-loading';
// Same shape of cycle (`compose -> threads -> thread-loading -> compose`), same
// reason it is safe: called at runtime, never at module init.
import { changeRows, focusThread, unfocusThread, versionsBeyondCurrent } from './threads';
import { abandonRowIntent, acknowledgeRowIntent, holdRowIntent } from '../rowIntents';
import { homeThreadId } from './homeThread';
import type { ChatContext } from './chatContext';
// Not a new edge at all: `./chat` above already imports this same function, so
// `compose -> pane` was in the graph before this line. The property to re-check
// is that nothing `pane` reaches comes back to `compose`. Its direct import list
// is not that check: it pulls in 25 action modules through the drawer.
import { revealThreadPane } from './pane';
import { markHashesAsSent } from '../../components/chat/pastedImages';
import { requestPromptCaretToEnd } from '../../components/chat/promptValueSync';
import { pushThreadNavState, removeThreadNavEntries } from './thread-navigation';

export type ComposeMode = 'lucidos' | 'claude_code';

function scopeEquals(a: Scope, b: Scope): boolean {
  if (a.kind !== b.kind) return false;
  if (a.kind === 'external' && b.kind === 'external') return a.repoId === b.repoId;
  if (a.kind === 'app' && b.kind === 'app') return a.appId === b.appId;
  return true;
}

/** Apply a picked compose destination: set the channel (global `inputMode` plus
 *  the composing draft's `mode`) and, for coding targets, the scope.
 *
 *  Each write is guarded so a same-value re-pick is a no-op, with no
 *  signal-identity churn, no debounced compose PUT and no SSE fan-out. The
 *  Dropdown fires onChange on every click, the already-selected option
 *  included. The draft patch only applies to a focused composing thread: once
 *  active, the channel is locked server-side and the picker is hidden. */
export function applyDestination(threadId: string | null, d: ComposeDestination): void {
  const mode: ComposeMode = d.kind === 'coding' ? 'claude_code' : 'lucidos';
  const modeType = mode === 'claude_code' ? 'coding_agent' : 'do';
  if (inputMode.value.type !== modeType) {
    inputMode.value = mode === 'claude_code' ? { type: 'coding_agent' } : { type: 'do' };
  }
  // Compose target: a focused composing draft (threadId), or the PENDING slot
  // for the not-yet-created draft (threadId null). An active thread has no
  // compose picker — ignore it defensively.
  const composing = !threadId || threadMap.value.get(threadId)?.meta.state === 'composing';
  if (!composing) return;
  // Per-draft (or pending) target. `patchComposeSelection(null, …)` routes to the
  // pending slot.
  if (d.kind === 'coding') {
    // Update the localStorage last-used scope seed (persisted by effects.ts) so
    // the NEXT new draft starts from this target. Leak-safe: `resolveScope`
    // reads `selectedScope` ONLY for the no-draft compose view, an existing
    // draft resolving its OWN stored scope, so this cannot move another draft.
    if (!scopeEquals(selectedScope.value, d.scope)) {
      selectedScope.value = d.scope;
    }
    const current = getComposeSelectionOverride(threadId).scope;
    if (!current || !scopeEquals(current, d.scope)) {
      patchComposeSelection(threadId, { scope: d.scope });
      // A scope change with no mode change would otherwise fire no compose PUT.
      // Persist the per-draft scope as a pending edit, which no report can
      // revert before a write covering it is answered.
      if (threadId) {
        noteLocalComposeEdit(threadId);
        schedulePush(threadId);
      }
    }
  }
  // Only a real composing draft has a per-draft mode to lock; the pending
  // draft's mode is seeded from `inputMode` at creation. null draft.mode is a
  // real change: the patch locks the pick server-side.
  if (threadId && getDraft(threadId).mode !== mode) {
    updateCompose(threadId, { mode });
  }
}

const DEFAULT_SCOPE: Scope = { kind: 'lucidos' };

const isOnRepo = (scope: Scope | undefined, repoIds: ReadonlySet<string>): boolean =>
  scope?.kind === 'external' && repoIds.has(scope.repoId);

/** The repository ids that a compose target points at and `registered` lacks.
 *  Only composing drafts count, since no other draft shows a picker. */
function unregisteredTargetRepoIds(registered: ReadonlySet<string>): Set<string> {
  const ids = new Set<string>();
  const note = (scope: Scope | undefined): void => {
    if (scope?.kind === 'external' && !registered.has(scope.repoId)) ids.add(scope.repoId);
  };
  note(selectedScope.value);
  note(pendingComposeSelection.value.scope);
  for (const [id, override] of composeSelections.value) {
    if (threadMap.value.get(id)?.meta.state === 'composing') note(override.scope);
  }
  return ids;
}

/** Drop every compose target on one of `goneRepoIds`, so a deleted repo stops
 *  being selected. A surface that showed it moves to the Lucidos Agent. That
 *  destination exists on every build, and it never re-aims a coding prompt at
 *  another codebase. */
export function dropRepoTargets(goneRepoIds: ReadonlySet<string>): void {
  // Each draft gets an explicit mode. A draft without one reads `inputMode`,
  // and would follow it back to the coding agent on the Lucidos source.
  for (const [id, override] of composeSelections.value) {
    if (!isOnRepo(override.scope, goneRepoIds) || threadMap.value.get(id)?.meta.state !== 'composing') continue;
    updateComposeSelection(id, { scope: DEFAULT_SCOPE });
    if (getDraft(id).mode !== 'lucidos') updateCompose(id, { mode: 'lucidos' });
  }
  if (isOnRepo(resolveScope(null), goneRepoIds) && inputMode.value.type === 'coding_agent') {
    inputMode.value = { type: 'do' };
  }
  if (isOnRepo(selectedScope.value, goneRepoIds)) selectedScope.value = DEFAULT_SCOPE;
  if (isOnRepo(pendingComposeSelection.value.scope, goneRepoIds)) {
    patchComposeSelection(null, { scope: DEFAULT_SCOPE });
  }
}

/** Keep compose targets on registered repositories. Re-checks whenever the
 *  repository list, a target, or a thread's state changes. A target missing
 *  from the cached list is reconfirmed with a refetch before anything drops.
 *  The cache can lag a repo another device just registered, and a draft reset
 *  syncs to every device.
 *
 *  Checks are coalesced into a microtask, so one runs only after a thread load
 *  has written a draft's selection, thread row and mode together. */
export function installUnregisteredRepoTargetReset(
  refetch: () => Promise<void> = refreshRepositories,
): () => void {
  let scheduled = false;
  let confirming = false;
  const registeredIds = (): Set<string> | null => {
    const repos = repositories.peek();
    return repos.status === 'loaded' ? new Set(repos.data.map(r => r.id)) : null;
  };
  const confirmAndDrop = async (): Promise<void> => {
    const cached = registeredIds();
    if (!cached || unregisteredTargetRepoIds(cached).size === 0) return;
    confirming = true;
    try {
      await refetch();
    } finally {
      confirming = false;
    }
    const fresh = registeredIds();
    const gone = fresh ? unregisteredTargetRepoIds(fresh) : new Set<string>();
    if (gone.size > 0) dropRepoTargets(gone);
  };
  return effect(() => {
    const repos = repositories.value;
    void composeSelections.value;
    void pendingComposeSelection.value;
    void selectedScope.value;
    void threadMap.value;
    if (repos.status !== 'loaded' || scheduled || confirming) return;
    scheduled = true;
    queueMicrotask(() => {
      scheduled = false;
      void confirmAndDrop();
    });
  });
}

interface ComposePatch {
  text?: string;
  image_hashes?: string[];
  mode?: ComposeMode;
}

export function currentComposeMode(): ComposeMode {
  return inputMode.value.type === 'coding_agent' ? 'claude_code' : 'lucidos';
}

/** Debounce window between keystrokes and the server PUT. Short enough that a
 *  peer device sees the change within an eye-blink. Long enough that a
 *  continuous typist does not flood the engine and SSE fan-out per character. */
const DEBOUNCE_MS = 250;

const pendingTimers = new Map<string, ReturnType<typeof setTimeout>>();

/** A thread's stored draft, as the composer compares it: text and images. */
export interface ServerDraft {
  text: string;
  imageHashes: string[];
}

/** Single entry point for compose mutations from the UI. Optimistic local
 *  apply, then debounced server PUT. A composing thread left empty by the patch
 *  is auto-discarded: never sent and with no content, it would otherwise render
 *  as a ghost row titled "Empty draft". updatedAt is deliberately left alone,
 *  typing a draft being no activity the drawer should surface. The backend's
 *  last-activity allowlist agrees. */
export function updateCompose(threadId: string, patch: ComposePatch): void {
  noteLocalComposeEdit(threadId);
  // Patch the draft first so the rollback path (state→composing on DELETE
  // failure) lands on the user's actual cleared state instead of stale text.
  patchDraft(threadId, patch);
  const thread = threadMap.value.get(threadId);
  // Auto-discard fires only when the patch CLEARED content — text or images.
  // A mode-only patch (the toggle click on an empty draft) is a "I'm preparing
  // to type, in this channel" signal, not a discard signal.
  const clearedContent = patch.text !== undefined || patch.image_hashes !== undefined;
  if (clearedContent && thread?.meta.state === 'composing' && draftIsEmpty(getDraft(threadId))) {
    void discardCompose(threadId);
    return;
  }
  schedulePush(threadId);
}

/** Single entry point for a per-draft dropdown selection change (model,
 *  reasoning, coding agent, coding-agent model and effort).
 *
 *  A real `threadId` patches the keyed override and schedules the debounced
 *  compose PUT. The pick is a pending edit, so no report applies over it until
 *  a write covering it is answered. A `null` id (fresh compose,
 *  no draft yet) writes only the pending slot, transferred and persisted when
 *  the draft is created. Scope has its own entry point (`applyDestination`),
 *  because it also updates the localStorage last-used seed. */
export function updateComposeSelection(
  threadId: string | null,
  patch: ComposeSelectionOverride,
): void {
  patchComposeSelection(threadId, patch);
  if (threadId) {
    noteLocalComposeEdit(threadId);
    schedulePush(threadId);
  }
}

/** Patch one thread's meta with whichever fields are set, returning a new
 *  Map so signal subscribers see the change. Used for lifecycle transitions
 *  (state changes, send/discard side effects) — never for draft text/images
 *  (those mutate per keystroke and live in `composeDrafts`). */
function mutateThreadMeta(threadId: string, patch: Partial<ThreadMeta>): void {
  const thread = threadMap.value.get(threadId);
  if (!thread) return;
  // Spread (not iterate-skip-undefined) so passing `{ field: undefined }`
  // explicitly clears the field — needed e.g. when sendCompose unbinds repoId
  // because the user picked the default repo on retry after a failed send.
  const next = new Map(threadMap.value);
  next.set(threadId, { ...thread, meta: { ...thread.meta, ...patch } });
  threadMap.value = next;
}

// --- Compose drafts: type locally, deliver durably ---
//
// A draft lives ONLY in the `composeDrafts` signal. The server is its storage,
// and the debounced PUT is the only thing that gets it there. Delivery is
// therefore the one thing that can still lose the user's text. On an installed
// iOS PWA it goes wrong for reasons that say nothing about the request: WebKit
// aborts every in-flight fetch when it suspends the page, and the tunnel to the
// engine drops on any radio change. Mirrors `pendingPreferenceWrites` in
// ./preferences.ts.

/** Threads whose latest draft the engine has not accepted, re-sent on the next
 *  resume / reconnect. See `docs/glossary.md` § Undelivered compose draft.
 *
 *  Distinct from a *pending intent* (`isComposeIntentPending`), which covers
 *  every edit the engine has not acknowledged. That one means "the engine has
 *  not confirmed this"; this one means "a write was owed and did not land".
 *
 *  Holds only the thread id, not a snapshot: the flush re-reads the CURRENT
 *  draft through `getDraft`, so last-write-wins is structural on the SEND side
 *  and no stale value can go out. An ANSWER always belongs to the newest intent,
 *  because writes for one thread never overlap: see `runComposePushes`. */
const undeliveredComposeDrafts = new Set<string>();

/** The engine REFUSED the PUT (4xx/5xx, a bad body). A verdict the user is owed
 *  and no retry can change. Keyed so repeats collapse into one card. */
const COMPOSE_REJECTED_TOAST = 'compose-sync-rejected';
/** The PUT never GOT to the engine, repeatedly. Keyed separately from the
 *  verdict above so draining the queue can retract this one without also
 *  clearing a rejection the user still needs to read. */
const COMPOSE_UNREACHABLE_TOAST = 'compose-sync-unreachable';

/** Consecutive pushes that got no ANSWER. Silent below the threshold, because
 *  the text is on screen and a re-send is owed, so one suspended fetch is noise
 *  rather than news. At the threshold it speaks once: a draft that is genuinely
 *  not reaching the engine must not be swallowed, since the page is the only
 *  other copy. Reset by any answer, a rejection included, since a 4xx proves the
 *  engine is reachable. Names no thread id: an id is meaningless to the user. */
const composePushFailures = createFailureCounter(3, () => {
  showToast(
    'Drafts are not reaching the engine. They are kept on this device and re-sent automatically.',
    'error',
    { key: COMPOSE_UNREACHABLE_TOAST },
  );
});

/** The engine answered about this thread. Both answers land here, accepted and
 *  refused alike, because both prove the engine is reachable. The unreachable
 *  banner must be retracted whichever way the queue drained, or it keeps
 *  insisting nothing is getting through while the rejection card says
 *  otherwise.
 *
 *  It can settle unconditionally because compose writes for one thread are
 *  serialized (see `runComposePushes`). The answer always belongs to the newest
 *  intent, no newer attempt having started while this one ran. */
function settleComposeDelivered(threadId: string): void {
  composePushFailures.recordSuccess();
  undeliveredComposeDrafts.delete(threadId);
  if (undeliveredComposeDrafts.size === 0) removeToast(COMPOSE_UNREACHABLE_TOAST);
}

/** Outcome of a compose PUT that did NOT succeed. A stale-epoch refusal never
 *  reaches here: it is not a failure, and `pushNow` handles it by adopting the
 *  epoch and re-queueing. */
function handleComposePushFailure(threadId: string, err: unknown): void {
  if (isTransientFetchError(err)) {
    // No answer: cancelled, timed out, or dropped in transit. Says nothing about
    // the request, and the user can see their text, so stay quiet and owe them a
    // re-send. Escalates once if this keeps happening.
    undeliveredComposeDrafts.add(threadId);
    composePushFailures.recordFailure();
    return;
  }
  // The engine ANSWERED and refused. No retry can change that and the user is
  // owed the reason: their local state has diverged from the server and they are
  // typing into thin air. It also proves the engine is reachable, so the failure
  // count resets and the thread stops being owed a re-send. The edit stays
  // pending: the engine refused to store it, but it is still the user's text,
  // and a report must not wipe it from under them.
  settleComposeDelivered(threadId);
  showToast(`Compose sync failed: ${errorDetail(err)}`, 'error', {
    key: COMPOSE_REJECTED_TOAST,
  });
}

/** Stop owing this thread a re-send, WITHOUT claiming the engine answered.
 *  For the paths where the draft itself stopped existing (sent or
 *  discarded), where a flush would push content the user is done with.
 *  Deliberately does not touch the failure counter: nothing was delivered. */
function dropUndeliveredComposeDraft(threadId: string): void {
  if (!undeliveredComposeDrafts.delete(threadId)) return;
  if (undeliveredComposeDrafts.size === 0) removeToast(COMPOSE_UNREACHABLE_TOAST);
}

/** Re-send every draft the engine never accepted. Called from `startClient`'s
 *  resume handler and from `runResumeSync` on reconnect, the two moments a
 *  suspended or disconnected client can reach the engine again. Re-enters
 *  through `schedulePush` rather than issuing its own request, so all the
 *  in-flight bookkeeping (and the re-park on another failure) stays in one
 *  place. A draft that fails again simply stays parked for the next resume. */
export function flushUndeliveredComposeDrafts(): void {
  if (undeliveredComposeDrafts.size === 0) return;
  // Snapshot first: `schedulePush` and its settle path mutate the set.
  for (const threadId of [...undeliveredComposeDrafts]) {
    // A thread that no longer exists (its POST /threads failed and rolled the
    // optimistic entry back) or one already discarded is owed nothing: `pushNow`
    // would early-return without ever settling, so the entry would sit here
    // forever and hold the unreachable card up with it. Drop rather than push.
    const thread = threadMap.peek().get(threadId);
    if (!thread || thread.meta.state === 'discarded') {
      dropUndeliveredComposeDraft(threadId);
      continue;
    }
    schedulePush(threadId);
  }
}

/** Test-only: the threads whose latest draft the engine has not accepted. */
export function _undeliveredComposeDraftsForTesting(): string[] {
  return [...undeliveredComposeDrafts].sort();
}

/** Test-only: drop every parked draft and the escalation counter, so one case's
 *  undelivered draft can't leak into the next. */
export function _resetUndeliveredComposeDraftsForTesting(): void {
  undeliveredComposeDrafts.clear();
  threadStarts.clear();
  runningComposePushes.clear();
  owedComposePushes.clear();
  _resetComposeSyncForTesting();
  composePushFailures.recordSuccess();
}

/** Threads with a compose write running RIGHT NOW. */
const runningComposePushes = new Set<string>();

/** Threads that owe another compose write once the running one settles. A set,
 *  not a queue: successive intents COALESCE. The write re-reads the draft when
 *  it goes out, so the only thing worth remembering is that one more is
 *  owed. */
const owedComposePushes = new Set<string>();

/** How many times one runner cycle re-issues after a stale-epoch refusal before
 *  giving up and parking the draft. A refusal means a submission consumed the
 *  slot, so the retry carries a strictly newer epoch and normally lands first
 *  try. The bound exists for the pathological case: a peer submitting over and
 *  over while we write. Spinning there is worse than waiting for the next
 *  resume flush. */
const MAX_STALE_EPOCH_RETRIES = 2;

function schedulePush(threadId: string): void {
  const existing = pendingTimers.get(threadId);
  if (existing) clearTimeout(existing);
  const t = setTimeout(() => {
    pendingTimers.delete(threadId);
    runComposePushes(threadId);
  }, DEBOUNCE_MS);
  pendingTimers.set(threadId, t);
}

/** Issue the thread's owed compose write, one at a time.
 *
 *  **Compose writes for one thread are serialized.** At most one PUT is in
 *  flight. An intent raised while one is running is recorded as owed and issued
 *  when that one settles, re-reading the draft at that moment. So last-write-
 *  wins means the last INTENT wins, which is what the rest of this file
 *  assumes.
 *
 *  Overlapping writes can be APPLIED out of order. On a stalled link that is
 *  how a pre-send draft ends up stored after the message that consumed it. The
 *  send goes through and the engine clears compose. The older write lands last
 *  and puts the draft back, and the next resync stages that stale revision into
 *  the composer. The engine's compose epoch refuses such a write outright.
 *  Serializing is the other half, and the half that also keeps two ordinary
 *  keystroke writes from landing backwards. */
function runComposePushes(threadId: string): void {
  owedComposePushes.add(threadId);
  if (runningComposePushes.has(threadId)) return;
  runningComposePushes.add(threadId);
  void (async () => {
    try {
      while (owedComposePushes.delete(threadId)) {
        await pushNow(threadId);
      }
    } catch (err) {
      // `pushNow` owns every outcome itself (see `handleComposePushFailure`),
      // so there is nothing left to reject. This is the unhandled-rejection
      // silencer for a genuine defect in that handling, not an error path.
      console.error('[compose] push outcome handling threw', err);
    } finally {
      runningComposePushes.delete(threadId);
      owedComposePushes.delete(threadId);
    }
  })();
}

/** Whether the engine is known to hold `current` as the draft's images. An
 *  unchanged array goes out as `null`, so the server's COALESCE preserves it
 *  and skips the SSE re-broadcast. */
function imageHashesUnchanged(threadId: string, current: string[]): boolean {
  const held = serverComposeOf(threadId)?.imageHashes;
  return held !== undefined && sameHashes(held, current);
}

/** The `mode` a compose write should carry. Only a composing draft has one to
 *  state: once sent, the channel is locked server-side and the picker is hidden.
 *  A pick the engine is known to hold goes out as `null`. Not knowing costs
 *  one redundant field and can never drop a channel pick.
 *
 *  Sending it on every keystroke is what turned an ordinary send race into a
 *  user-facing card. A write composed before the send arrived after it. The
 *  engine then read a mode it never needed as an attempt to change a sent
 *  thread's channel. See
 *  `docs/plans/2026-08-26-compose-mode-lock-masks-the-stale-write-fence.md`. */
function composeModeForWrite(
  threadId: string,
  state: ThreadMeta['state'],
  draft: ComposeDraft,
): ComposeMode | null {
  if (state !== 'composing') return null;
  const held = serverComposeOf(threadId);
  if (held && held.mode === draft.mode) return null;
  return draft.mode;
}

/** One compose write, reading the draft at the moment it goes out. Only ever
 *  called from `runComposePushes`, which guarantees no sibling write is in
 *  flight for the same thread. `staleRetries` counts the re-issues this runner
 *  cycle has already spent adopting a newer epoch. */
async function pushNow(threadId: string, staleRetries = 0): Promise<void> {
  try {
    // PUT 404s if the 250ms debounce elapses before POST /threads settles.
    await awaitThreadStarted(threadId);
  } catch {
    // `startThreadOnEngine` already reported a refused start, and one that got
    // no answer is owed and re-sent by the flush. A toast here would repeat it.
    return;
  }
  const thread = threadMap.value.get(threadId);
  // Discarded threads stay in threadMap with state='discarded' until the
  // SSE confirms the DELETE, so existence alone is not enough: a PUT here
  // would 410 against a thread the user already discarded.
  if (!thread || thread.meta.state === 'discarded') return;
  // Read with the draft, so the ack covers exactly the edits this write sends.
  const seq = composeEditSeq(threadId);
  const draft = getDraft(threadId);
  // null = preserve via COALESCE (avoids the SSE re-broadcast).
  const wireHashes: string[] | null = imageHashesUnchanged(threadId, draft.image_hashes)
    ? null
    : [...draft.image_hashes];
  // Persist the draft's per-draft dropdown selection alongside text/images/mode
  // so a reload rehydrates it. `undefined` (no local selection) omits the field
  // → backend COALESCE preserves. A present selection replaces the stored one;
  // re-sending the same object on a plain keystroke PUT is a harmless no-op.
  const selectionOverride = getComposeSelectionOverride(threadId);
  const selectionForPut: ComposeSelectionOverride | undefined =
    Object.keys(selectionOverride).length > 0 ? selectionOverride : undefined;
  // null = preserve via COALESCE, so a pick the engine already holds costs
  // nothing and cannot trip the server's mode lock.
  const wireMode = composeModeForWrite(threadId, thread.meta.state, draft);
  let result: ComposePutResult;
  try {
    result = await putComposeOnThread(
      threadId,
      draft.text,
      wireHashes,
      wireMode,
      selectionForPut,
      knownComposeEpoch(threadId),
    );
  } catch (err) {
    handleComposePushFailure(threadId, err);
    return;
  }
  if (result.status === 'stale') {
    // A submission consumed the compose slot after this write was composed,
    // so the engine dropped it. Nothing is wrong and nothing is owed to the
    // user: record the state it reported, which carries the epoch, and
    // re-issue. The edit stays pending, so the report cannot replace it.
    acceptComposeReport(threadId, result.report);
    if (staleRetries < MAX_STALE_EPOCH_RETRIES) {
      await pushNow(threadId, staleRetries + 1);
      return;
    }
    // Something keeps consuming the slot faster than we can write. Park it
    // rather than spin; the next resume flush re-reads the current draft.
    undeliveredComposeDrafts.add(threadId);
    return;
  }
  // An engine one release behind answers without a report. The write still
  // landed, so it says what we sent, at a revision nobody named.
  const held = serverComposeOf(threadId);
  const report: ComposeReport = result.report ?? {
    revision: null,
    epoch: null,
    text: draft.text,
    imageHashes: [...draft.image_hashes],
    mode: wireMode ?? held?.mode ?? null,
    selection: selectionForPut ?? held?.selection ?? null,
  };
  acknowledgeComposeEdits(threadId, seq, report);
  settleComposeDelivered(threadId);
}

/** Forget every per-thread compose record, for a thread that will never be
 *  fetched again.
 *
 *  `dropUndeliveredComposeDraft` is the load-bearing one. Leave the id in that
 *  set and the next resume re-sends a draft for a thread that is gone. That
 *  pins the unreachable-drafts toast until a flush collects it.
 *
 *  Two callers: discarding a draft, and deleting the thread (ADR 0192). The
 *  discard can be rolled back, so it snapshots what its restore cannot
 *  re-derive, the epoch fence included.
 */
export function forgetComposeState(threadId: string): void {
  cancelPendingPush(threadId);
  clearDraft(threadId);
  clearComposeSelection(threadId);
  forgetComposeSync(threadId);
}

function cancelPendingPush(threadId: string): void {
  const t = pendingTimers.get(threadId);
  if (t) {
    clearTimeout(t);
    pendingTimers.delete(threadId);
  }
  // A debounce is no longer the only place an uncommitted intent waits: one
  // whose timer already fired while a write was running sits QUEUED instead,
  // with no timer to clear. Both callers are dropping the draft (send consumed
  // it, discard destroyed it), so a queued write for it is at best redundant.
  owedComposePushes.delete(threadId);
  // Shared teardown for send and discard, so both stop owing a re-send here.
  // Unconditional (outside the timer branch): a push that already failed has no
  // timer left, and that is exactly the thread a later flush would resurrect.
  dropUndeliveredComposeDraft(threadId);
}

/** Idempotent: POST /threads with `{id, mode}`. No-op if the thread is already
 *  in threadMap. The server is idempotent on the same `{id, mode}` too, but
 *  skipping the round trip matters for first-keystroke latency. Inserts an
 *  optimistic composing entry, so the UI has somewhere to write before the ack
 *  lands, plus a draft entry holding the user's mode pick. */
export async function startComposeIfNeeded(threadId: string, mode: ComposeMode): Promise<void> {
  if (threadMap.value.has(threadId)) return;
  const next = new Map(threadMap.value);
  next.set(threadId, makeOptimisticThreadState({
    id: threadId,
    title: '',
    channel: mode === 'claude_code' ? 'claude_code' : 'chat',
    initiator: 'user',
    eventsLoaded: true,
    state: 'composing',
    status: 'idle',
  }));
  threadMap.value = next;
  setDraft(threadId, { text: '', image_hashes: [], mode });
  await startThreadOnEngine(threadId, mode);
}

/** Per draft, the `POST /threads` the engine has not confirmed yet: one in
 *  flight, or one owed after a transient failure. No entry means the row
 *  exists, or this client never started it. */
type ThreadStart =
  | { kind: 'in-flight'; promise: Promise<void> }
  | { kind: 'owed'; mode: ComposeMode };

const threadStarts = new Map<string, ThreadStart>();

/** One `POST /threads` for an optimistic draft, single-flight per thread.
 *
 *  A transient failure keeps the draft, its images and its row: an engine that
 *  did not answer has said nothing about the draft. The start is then owed,
 *  and the draft joins the undelivered set, so the reconnect flush retries it.
 *  Only a verdict rolls the optimistic row back. It is reported here, so a
 *  refused re-send is heard whichever caller needed the row. */
function startThreadOnEngine(threadId: string, mode: ComposeMode): Promise<void> {
  const current = threadStarts.get(threadId);
  if (current?.kind === 'in-flight') return current.promise;
  const promise = (async () => {
    let started: ComposeReport | null;
    try {
      started = await ensureThreadStarted(threadId, mode);
    } catch (err) {
      // Live, not `composing`: `sendCompose` marks the row active before it
      // waits on the start, and a send that got no answer keeps its draft too.
      const thread = threadMap.value.get(threadId);
      const live = thread !== undefined && thread.meta.state !== 'discarded';
      if (live && isTransientFetchError(err)) {
        threadStarts.set(threadId, { kind: 'owed', mode });
        undeliveredComposeDrafts.add(threadId);
        composePushFailures.recordFailure();
        throw err;
      }
      threadStarts.delete(threadId);
      rollbackOptimistic(threadId);
      if (live) reportRefusedThreadStart(threadId, err);
      throw err;
    }
    threadStarts.delete(threadId);
    // The row now holds this mode: `ThreadStarted` stores it, and the endpoint
    // 409s rather than accepting a different one. So the first keystroke write
    // has nothing to state about the channel. An engine answering without a
    // report still stored exactly that.
    acceptComposeReport(threadId, started ?? {
      revision: null, epoch: null, text: '', imageHashes: [], mode, selection: null,
    });
  })();
  threadStarts.set(threadId, { kind: 'in-flight', promise });
  return promise;
}

/** The engine refused to start a draft's thread, and the row is gone. Drop the
 *  nav entry so Forward cannot restore an id with no row, and the focus. */
function reportRefusedThreadStart(threadId: string, err: unknown): void {
  removeThreadNavEntries(threadId);
  if (focusedThreadId.value === threadId) setFocusedThread(null);
  showToast(`Failed to start compose: ${errorDetail(err)}`, 'error');
}

/** True while the engine has not confirmed this draft's thread row. */
export function isThreadStartPending(threadId: string): boolean {
  return threadStarts.has(threadId);
}

/** True when this draft's start failed transiently and nothing has re-sent it. */
function isThreadStartOwed(threadId: string): boolean {
  return threadStarts.get(threadId)?.kind === 'owed';
}

function rollbackOptimistic(threadId: string): void {
  if (!threadMap.value.has(threadId)) return;
  const next = new Map(threadMap.value);
  next.delete(threadId);
  threadMap.value = next;
  // The optimistic row is inserted with `eventsLoaded: true`, so a wake or an
  // SSE resync during the `ensureThreadStarted` window can record a verdict
  // against it. Removing the row owes those maps the same cleanup as
  // `sendMessage`'s rollback: nothing will ever fetch this thread again.
  forgetThreadEventsFailures(threadId);
  clearDraft(threadId);
  forgetComposeSync(threadId);
  // `ensureFocusedComposeThread` seeded this id's scope before the POST went
  // out. Leaving it behind keeps an override for a thread that never existed.
  clearComposeSelection(threadId);
}

/** Resolve once the engine holds this draft's thread row. Callers that need
 *  the row before their own request wait here: the draft PUT, the send, and
 *  the image upload, which fires with no debounce to hide the race.
 *
 *  An owed start is re-sent here, so whoever needs the row next retries it.
 *  Resolves at once when nothing is pending, and rejects if the start fails. */
export async function awaitThreadStarted(threadId: string): Promise<void> {
  const start = threadStarts.get(threadId);
  if (!start) return;
  if (start.kind === 'in-flight') return start.promise;
  return startThreadOnEngine(threadId, start.mode);
}

/** Lazy-create a thread id when the user starts composing without a thread
 *  focused. The id is allocated client-side; `startComposeIfNeeded` POSTs the
 *  row server-side. A refused start rolls back and toasts, and one that got no
 *  answer keeps the draft and stays owed (`startThreadOnEngine`). */
export function ensureFocusedComposeThread(): string {
  let id = focusedThreadId.value;
  if (id) return id;
  id = generateUuid();
  setFocusedThread(id);
  // Seed THIS new draft's own stored selection. Eager-copy the localStorage
  // last-used scope, so the draft carries a scope in its OWN override, then
  // overlay any fresh-compose picks from the pending slot. A pending scope pick
  // wins. The eager copy is required: `resolveScope` no longer falls back to
  // the shared `selectedScope` for a real draft, which is the leak guard. Other
  // fields stay unset and resolve to their account defaults.
  seedComposeSelection(id, { scope: selectedScope.value, ...takePendingComposeSelection() });
  // Inlined instead of focusThread(): that also fires loadThreadEvents and
  // (on mobile) navigateToPane, neither of which the draft path wants.
  pushThreadNavState({ type: 'thread', id });
  startComposeIfNeeded(id, currentComposeMode()).catch(() => {
    // Already handled: a refused start reports itself in `startThreadOnEngine`,
    // and one that got no answer is owed and re-sent.
  });
  // The seeded picks are a local edit, written at once. Pending, the thread
  // start's report cannot wipe them, and the write's answer releases them.
  noteLocalComposeEdit(id);
  schedulePush(id);
  return id;
}

/** Prefill the compose input with a seeded prompt, today the setup interview's
 *  sentence. Lazily focuses or creates a composing thread. The text then goes
 *  through the normal debounced compose path, so it syncs to the textarea and
 *  persists like any typed draft.
 *
 *  Replaces the WHOLE input, text and any attached images, so the seeded prompt
 *  lands cleanly. A lingering attachment would otherwise ride along with an
 *  unrelated sentence. Does NOT send: the user reviews, picks a destination,
 *  and hits Send. Returns the thread id the text landed on.
 *
 *  Lands on whatever `ensureFocusedComposeThread` resolves, which is the
 *  FOCUSED thread when there is one, active threads included. A caller that
 *  must not write into an already-sent thread calls `dropNonComposingFocus()`
 *  first. `seedSuggestion` does, and has to do it there rather than here,
 *  because it reads the target before prefilling. */
export function prefillCompose(text: string): string {
  const threadId = ensureFocusedComposeThread();
  updateCompose(threadId, { text, image_hashes: [] });
  return threadId;
}

/** Release the focus when it points at a thread that has already been sent.
 *  The next `ensureFocusedComposeThread` then allocates a fresh draft instead
 *  of returning the open thread.
 *
 *  `ensureFocusedComposeThread` hands back the focused id whatever its state.
 *  That is right for typing, an active thread's composer writing a follow-up
 *  draft onto that thread. It is wrong for anything that COMPOSES a message on
 *  the user's behalf. The setup interview is the sharp case: its header button
 *  is permanent, so it can be tapped with any thread focused. Without this it
 *  aims the interview at whatever the user was looking at.
 *  `handleNavigationRequest`'s `new-chat` branch drops focus for this reason.
 *
 *  A focused DRAFT is left alone here, because emptiness is what decides its
 *  fate and this function cannot see it. `seedSuggestion` reads the draft and
 *  steps off an occupied one itself. `unfocusThread` rather than a bare
 *  `setFocusedThread(null)`, so the leaving thread's coding-agent pending picks
 *  are reset and the thread pane is revealed. That is how a mobile user tapping
 *  the header button reaches the conversation that starts. */
function dropNonComposingFocus(): void {
  const id = focusedThreadId.value;
  if (!id) return;
  if (threadMap.value.get(id)?.meta.state === 'composing') return;
  unfocusThread();
}

/** Seed a suggested sentence into a composer, and return the draft it landed
 *  on.
 *
 *  Its one caller is {@link sendSeededPrompt}, which reuses every part of it and
 *  only adds the send. It stays a separate step so the seeding stays testable
 *  without a message going out.
 *
 *  **A draft in progress is stepped off, never overwritten.** The sentence is
 *  sent immediately, so it has no use for the user's composer: an occupied one
 *  is released and `prefillCompose` allocates a fresh draft. Their text, their
 *  attachments and their destination are left exactly as they were.
 *
 *  That replaced a "Replace it with this suggestion?" confirm, and the confirm
 *  is what made the loss possible in the first place. It also cost the reader a
 *  decision about their own work that this never has to ask. An EMPTY focused
 *  draft is still reused, since there is nothing to protect and a second empty
 *  row is litter.
 *
 *  A suggested sentence is conversational, so the destination is forced to the
 *  Lucidos Agent, a coding-agent draft flipping back to chat.
 *
 *  The seeded text replaces the draft, so the caret end-snaps
 *  (`requestPromptCaretToEnd`): any old offset indexes text that is gone, and
 *  restoring it would drop the user inside the seeded sentence.
 *
 *  Does NOT send: that is {@link sendSeededPrompt}'s extra step. */
export function seedSuggestion(text: string): string {
  stepOffDraftInProgress();
  // Target the Lucidos Agent. Set BEFORE prefill so a brand-new draft is born on
  // the chat channel, and so an existing coding-agent draft flips back to chat.
  applyDestination(focusedThreadId.value, { kind: 'lucidos-agent' });
  const threadId = prefillCompose(text);
  requestPromptCaretToEnd();
  return threadId;
}

/** Put text the engine refused back where it can be sent again: a fresh draft
 *  on the destination the compose view would pick. A draft in progress is
 *  stepped off, never overwritten, as in `seedSuggestion`. */
export function composeInFreshDraft(text: string, imageHashes: string[]): void {
  stepOffDraftInProgress();
  const threadId = prefillCompose(text);
  if (imageHashes.length > 0) updateCompose(threadId, { image_hashes: imageHashes });
  requestPromptCaretToEnd();
}

/** Leave the focus on an empty draft or on nothing. The next prefill then
 *  neither lands on a sent thread nor overwrites text the user is writing. */
function stepOffDraftInProgress(): void {
  dropNonComposingFocus();
  const existingId = focusedThreadId.value;
  if (existingId && !draftIsEmpty(getDraft(existingId))) unfocusThread();
}

/** The message the setup-interview entry points send.
 *
 *  Deliberately an ordinary English sentence rather than a magic token. It
 *  lands in the transcript as the user's own message, so the mechanism is
 *  visible and they can reword it later without the button. That is the
 *  prompt-first side of `docs/philosophy.md` principle 3, applied to the one
 *  surface a newcomer meets first.
 *
 *  **The phrase "help me get the most out of Lucidos" is load-bearing.** The
 *  chat system prompt's `SETUP_INTERVIEW_RULE` keys on it to route the turn at
 *  `load_knowhow('system-knowhow/setup-interview')`. The knowhow's own
 *  frontmatter `description` repeats it for the retrieval path. All three are
 *  pinned together by
 *  `setup_interview_route_matches_the_frontend_seeded_prompt`, which reads THIS
 *  file. Reword the sentence freely, but keep that clause.
 *
 *  "my work and my life" rather than "my work and my week" is deliberate. The
 *  interview covers personal admin, training and learning on the same footing
 *  as a job (see `system-knowhow/setup-interview.md`, rung 1). The sentence the
 *  user watches themselves send should not narrow it back down. */
export const SETUP_INTERVIEW_PROMPT =
  'Help me get the most out of Lucidos: interview me about my work and my life, '
  + 'then build me the apps and automations that fit, here in my workspace.';

/** Start the setup interview: send {@link SETUP_INTERVIEW_PROMPT} as the user.
 *
 *  It does not stop at a draft. A first-run user staring at a prefilled box
 *  they did not write has to decide whether to send it. That hesitation is
 *  what the entry point exists to remove, so the click is the whole gesture.
 *  Nothing is hidden by sending: the sentence is what appears in the
 *  transcript, on the same code path a typed message takes.
 *
 *  It runs in the home thread, where everything starts (ADR 0411), so it
 *  creates no thread of its own. Only a workspace whose Home could not be
 *  created starts it in a new thread, as a seeded prompt.
 *
 *  Returns true when the interview was sent, false when the send failed. */
export async function startSetupInterview(): Promise<boolean> {
  const home = homeThreadId.value;
  if (!home) return sendSeededPrompt(SETUP_INTERVIEW_PROMPT, 'start the setup interview');
  focusThread(home);
  try {
    return (await sendMessage(SETUP_INTERVIEW_PROMPT, undefined, { threadId: home })) === 'sent';
  } catch (err) {
    showToast(`Failed to start the setup interview: ${errorDetail(err)}`, 'error');
    return false;
  }
}

/** Seed `text` into the composer and SEND it, as the user.
 *
 *  The gesture behind every button that starts a conversation on the user's
 *  behalf: the first-run welcome's setup interview, a *release notice* action,
 *  and Discuss on a notification or on either webhook bar. Each hands the
 *  reader a sentence they can see, reword and re-send by typing, which is the
 *  prompt-first side of `docs/philosophy.md` principle 3.
 *
 *  It never disturbs a draft in progress: `seedSuggestion` steps onto a fresh
 *  one rather than borrowing an occupied composer.
 *
 *  It reveals the thread pane, because a send that starts a conversation lands
 *  on a thread (`.claude/rules/frontend.md` § Navigation That Lands Content).
 *  `seedSuggestion` covers only the branch where it unfocuses, and `sendCompose`
 *  passes an explicit thread id, so `sendMessage`'s raw-new reveal never fires.
 *  Without this, a caller in the content pane sent into a thread the user could
 *  not see.
 *
 *  `what` completes "Failed to …" in the error toast, so write it as a verb
 *  phrase. Returns false when the send failed. */
export async function sendSeededPrompt(text: string, what: string): Promise<boolean> {
  const threadId = seedSuggestion(text);
  // Before the send, not after. `ensureFocusedComposeThread` allocated the id
  // client-side, so the thread can be on screen while the request is still in
  // flight, rather than after a round trip.
  revealThreadPane();
  try {
    return await sendCompose(threadId, { focus: true });
  } catch (err) {
    // `sendCompose` rethrows after rolling the draft back, and its other callers
    // toast (see `beginSend` in PromptInput). Every entry point here fires this
    // as `void`, so swallowing would surface a failed click as nothing
    // happening at all.
    showToast(`Failed to ${what}: ${errorDetail(err)}`, 'error');
    return false;
  }
}

/** Discard a composing thread. The row shows discarded at once, as a row
 *  intent held until the row reaches the DELETE's version (ADR 0437). So no
 *  read from before the discard brings the draft back. 404/410
 *  (already gone server-side) is the desired end-state — swallow. Other
 *  errors roll back and toast.
 *  Releases focus if the discarded thread was the focused one so the next
 *  keystroke lazy-creates a fresh draft via ensureFocusedComposeThread.
 *  The draft entry is dropped; if DELETE fails we restore it to the
 *  pre-discard text AND its destination, and write it back, so the user loses
 *  neither what they typed nor where they aimed it. */
export async function discardCompose(threadId: string): Promise<void> {
  if (focusedThreadId.value === threadId) setFocusedThread(null);
  const restoreDraft = snapshotDraft(threadId);
  // A draft is text AND a destination, so both are snapshotted. Copied,
  // because the rollback must not alias an object a later pick could mutate.
  const restoreSelection: ComposeSelectionOverride = { ...getComposeSelectionOverride(threadId) };
  changeRows([threadId], (meta) => {
    holdRowIntent(meta, 'discard', { state: 'discarded' });
    return true;
  });
  // The emptied draft is a pending edit until the DELETE answers, so a write
  // or report already in flight cannot put it back meanwhile. The compose
  // record is forgotten only once the engine has answered.
  noteLocalComposeEdit(threadId);
  cancelPendingPush(threadId);
  clearDraft(threadId);
  clearComposeSelection(threadId);
  // Pairs with the push in ensureFocusedComposeThread. Back and Forward must
  // not restore a discarded thread whose events would 404.
  removeThreadNavEntries(threadId);
  if (isThreadStartOwed(threadId)) {
    // The engine never held this draft. There is nothing to delete there, and
    // no discard event will arrive to clear the local row.
    threadStarts.delete(threadId);
    changeRows([threadId], (meta) => abandonRowIntent(meta, 'discard'));
    rollbackOptimistic(threadId);
    return;
  }
  const legacyVersion = versionsBeyondCurrent([threadId]).get(threadId);
  const settleDiscard = (version: number | undefined) => changeRows([threadId], (meta) => version === undefined
    ? abandonRowIntent(meta, 'discard')
    : acknowledgeRowIntent(meta, 'discard', version));
  try {
    settleDiscard((await deleteThread(threadId)) ?? legacyVersion);
    forgetComposeState(threadId);
  } catch (err) {
    if (isAlreadyGone(err)) {
      settleDiscard(legacyVersion);
      forgetComposeState(threadId);
      return;
    }
    changeRows([threadId], (meta) => abandonRowIntent(meta, 'discard'));
    // Unsent local work again: restore it, and write it back so the engine
    // acknowledges it and the edit stops pending.
    noteLocalComposeEdit(threadId);
    if (restoreDraft) setDraft(threadId, restoreDraft);
    // The destination rides back with the text. Without it `resolveScope`
    // answers Lucidos, and Send then runs the coding agent against the Lucidos
    // source rather than the repo the user picked.
    seedComposeSelection(threadId, restoreSelection);
    schedulePush(threadId);
    showToast(`Discard failed: ${errorDetail(err)}`, 'error');
  }
}

/** Snapshot a draft for rollback. Returns undefined when no entry exists so
 *  the rollback path doesn't seed an empty draft on a thread that had none. */
function snapshotDraft(threadId: string): ComposeDraft | undefined {
  const draft = composeDrafts.value.get(threadId);
  return draft && { ...draft, image_hashes: [...draft.image_hashes] };
}

function isAlreadyGone(err: unknown): boolean {
  return err instanceof ApiError && (err.httpCode === 404 || err.httpCode === 410);
}

/** The last thing a send owes the engine: one compose write carrying the
 *  cleared draft. Writes are serialized, so this is issued only after every
 *  earlier write for the thread has settled. It is therefore the last one the
 *  engine applies, and a pre-send draft PUT can never be the resting state.
 *
 *  `sendCompose` calls this. `sendFollowup` reaches the same guarantee through
 *  its `updateCompose(id, {text: ''})`, which clears the draft locally and
 *  schedules the same write. Two entry points rather than one, because the
 *  follow-up path owes a local clear as well. Routing it through here too would
 *  schedule a second, redundant write.
 *
 *  It goes through the ordinary debounced path rather than a bespoke request,
 *  so it re-reads the draft when it fires. That makes the awkward case right
 *  for free: type a new follow-up straight after sending, and this write
 *  carries the new text instead of an empty draft. */
function pushClearedComposeAfterSend(threadId: string): void {
  schedulePush(threadId);
}

/** A discarded unsent first send leaves its thread an empty draft, which is
 *  what the engine holds. */
export function reopenEmptyDraft(threadId: string): void {
  mutateThreadMeta(threadId, { state: 'composing' });
}

/** A first send restored after a reload, on a thread the engine may still
 *  have composing. Show it as sent, as `sendCompose` did. The engine's draft
 *  may still be the one the message was written from. The composer must not
 *  show that beside the message, so it is cleared here and on the engine. A
 *  draft that changed since was typed after the send, and stays. */
export function resumeUnsentFirstSend(threadId: string, engineDraftAtSend: ServerDraft | undefined): void {
  if (threadMap.value.get(threadId)?.meta.state === 'composing') mutateThreadMeta(threadId, { state: 'active' });
  const onEngine = serverComposeOf(threadId);
  const local = getDraft(threadId);
  const stale = engineDraftAtSend !== undefined && onEngine !== undefined
    && sameServerDraft(onEngine, engineDraftAtSend)
    && sameServerDraft(onEngine, { text: local.text, imageHashes: local.image_hashes });
  if (stale && !draftIsEmpty(local)) updateCompose(threadId, { text: '', image_hashes: [] });
}

function sameServerDraft(a: ServerDraft, b: ServerDraft): boolean {
  return a.text === b.text && sameHashes(a.imageHashes, b.imageHashes);
}

/** What a compose draft's first message owes once the engine takes it. */
export function settleAcceptedFirstSend(threadId: string): void {
  // A compose-view pick is a one-shot intent, now carried into this spawn's
  // chat body, so consume the draft's selection. Without this the override
  // would linger in `composeSelections` for a thread no longer composing.
  clearComposeSelection(threadId);
  // Scheduled AFTER the selection was consumed, for two reasons. The write's
  // answer is what acknowledges the send's clear, so the edit stops pending.
  // And the order keeps the write from carrying the draft's picks back onto a
  // row whose `compose_selection` the accept just cleared (ADR 0450).
  pushClearedComposeAfterSend(threadId);
}

/** Put a refused first send back in its draft. Typing that landed meanwhile
 *  keeps its place, and the refused text and images go after it, as a refused
 *  follow-up does. Neither copy may be dropped: the optimistic row is gone, so
 *  the draft is all that holds the refused message. The draft's picks were
 *  never consumed. */
export function rollBackFirstSend(
  threadId: string,
  sent: { text: string; imageHashes: string[]; mode: ComposeMode | null },
): void {
  mutateThreadMeta(threadId, { state: 'composing' });
  const current = getDraft(threadId);
  const typedMeanwhile = current.text.trim().length > 0;
  const restore: Partial<ComposeDraft> = {};
  if (!typedMeanwhile) restore.text = sent.text;
  else if (sent.text.length > 0) restore.text = `${current.text}\n\n${sent.text}`;
  const missingImages = sent.imageHashes.filter(h => !current.image_hashes.includes(h));
  if (missingImages.length > 0) restore.image_hashes = [...current.image_hashes, ...missingImages];
  if (current.mode === null) restore.mode = sent.mode;
  if (Object.keys(restore).length === 0) return;
  patchDraft(threadId, restore);
  // A replaced draft end-snaps the caret. An appended one keeps it, since the
  // prefix it points into is untouched.
  if (current.text.length > 0 && restore.text !== undefined && !typedMeanwhile) requestPromptCaretToEnd();
  // The restored draft is unsent work again, and the send dropped the write
  // that would have stored its last keystrokes. Owe the engine that write, or
  // a reload brings back the older text.
  noteLocalComposeEdit(threadId);
  schedulePush(threadId);
}

/** Send the focused thread's current compose contents as the first message.
 *  Reads text/images from the draft signal so the caller doesn't need to
 *  pass them. Optimistic local clear + state→active before the chat POST so
 *  the input is immediately usable for follow-up text. On failure we restore
 *  the typed text, because losing it would be the worst possible UX.
 *
 *  Resolves false when the engine refused the send, after its toast and the
 *  restore. Rejects, after the restore, when the thread could not start. */
export async function sendCompose(
  threadId: string,
  opts: { useCodingAgent?: boolean; context?: ChatContext | null; focus?: boolean },
): Promise<boolean> {
  // Both guards below end a send the user asked for, so neither may be silent.
  // A dispatched send that produces no message and no word is a dead button, and
  // this is the path a compose-view Send takes. See
  // docs/plans/2026-08-27-the-composer-sends-the-draft-it-is-showing.md.
  const thread = threadMap.value.get(threadId);
  if (!thread) {
    showToast('Could not send: that draft is no longer open.', 'error');
    return false;
  }
  const draft = getDraft(threadId);
  const text = draft.text;
  const wireHashes = draft.image_hashes;
  const mode = draft.mode;
  const onEngineAtSend = serverComposeOf(threadId);
  const engineDraftAtSend: ServerDraft | undefined = onEngineAtSend
    && { text: onEngineAtSend.text, imageHashes: [...onEngineAtSend.imageHashes] };
  if (!text.trim() && wireHashes.length === 0) {
    showToast('Could not send: the saved draft was empty.', 'error');
    return false;
  }

  cancelPendingPush(threadId);
  // Bind here so sendMessage need not detect first-send from follow-up (see
  // frontend.md "Drafts Are Threads"). Every dropdown value resolves from THIS
  // draft's override in `composeSelections`, falling back to the global
  // default. Never read straight off a global signal another draft may have
  // changed.
  //
  // Channel is locked from `opts.useCodingAgent`, which the caller resolved via
  // effectiveSendMode, rather than from the existing meta.channel. That was
  // stamped at first-keystroke time and goes stale the moment the user toggles.
  // Without the lock, sendMessage reads the stale channel and routes a
  // coding-agent send through the Lucidos Agent, or the reverse.
  const scope = resolveScope(threadId);
  const boundRepoId = opts.useCodingAgent && scope.kind === 'external' ? scope.repoId : undefined;
  const boundCodingAgentKind = opts.useCodingAgent ? scope.kind : undefined;
  const boundCodingAgentFolder = opts.useCodingAgent && scope.kind === 'app'
    ? `data/apps/${scope.appId}`
    : undefined;
  const boundChannel: ThreadMeta['channel'] = opts.useCodingAgent ? 'claude_code' : 'chat';
  const boundCodingAgent = opts.useCodingAgent ? resolveCodingAgent(threadId) : undefined;
  // The destination's NAME is part of the binding, not just its ids. The drawer
  // row chips `meta.repoName` once the thread is started, and the engine
  // reports it as `cc_repo_name` only a round trip later. Binding the ids alone
  // therefore makes the chip vanish and reappear. `scopeRepoName` is the same
  // resolution the draft row used, so the two frames agree. The engine's answer
  // overwrites it when it lands.
  const boundRepoName = opts.useCodingAgent
    ? scopeRepoName(scope, loadedOr(repositories.value, []))
    : undefined;
  mutateThreadMeta(threadId, {
    state: 'active',
    channel: boundChannel,
    repoId: boundRepoId,
    repoName: boundRepoName,
    codingAgentKind: boundCodingAgentKind,
    codingAgentFolder: boundCodingAgentFolder,
    codingAgent: boundCodingAgent,
  });
  // Resolve the per-draft model/effort picks and pass them to sendMessage so the
  // send uses THIS draft's selection, not a global another draft may have
  // changed. The Lucidos Agent path uses model/reasoningEffort; the coding-agent
  // path uses ccModel/ccReasoningEffort. sendMessage falls back to the globals
  // when an override is absent (raw-new sends + follow-ups keep the old path).
  const modelOverride = resolveModel(threadId);
  const reasoningEffortOverride = resolveReasoningEffort(threadId) ?? undefined;
  const providerOverride = resolveProvider(threadId) ?? undefined;
  const ccModelOverride = resolveCcModel(threadId);
  const ccReasoningEffortOverride = resolveCcReasoningEffort(threadId);
  // Must run before the draft clear — see `markHashesAsSent`.
  if (wireHashes.length > 0) markHashesAsSent(wireHashes);
  // The clear stays a pending edit until the write the send owes is answered.
  // So no report from before the send can put the sent text back.
  noteLocalComposeEdit(threadId);
  clearDraft(threadId);
  const shouldFocus = opts.focus ?? true;
  if (shouldFocus) setFocusedThread(threadId);
  const rollBack = () => rollBackFirstSend(threadId, { text, imageHashes: wireHashes, mode });
  try {
    // The chat POST needs the thread row to exist server-side, and on a
    // first-send `POST /threads` may still be in flight.
    // `ensureFocusedComposeThread` fires it without awaiting. Every optimistic
    // local step above stays synchronous, since the input must clear on the
    // gesture. `sendMessage` waits for it after its optimistic row, so the
    // composer shows a turn in flight meanwhile, never an idle thread.
    //
    // A button that composes and sends in one gesture is the case that needs
    // it. Typing hides the race, the draft PUT awaiting the same promise in
    // `pushNow`, and `cancelPendingPush` above has just dropped that PUT. A
    // failed start rejects out of `sendMessage` into the catch below, which
    // rolls the draft back and rethrows for the caller to toast.
    const outcome = await sendMessage(text, wireHashes.length > 0 ? wireHashes : undefined, {
      useCodingAgent: opts.useCodingAgent,
      context: opts.context,
      threadId,
      focus: shouldFocus,
      modelOverride,
      reasoningEffortOverride,
      providerOverride,
      ccModelOverride,
      ccReasoningEffortOverride,
      threadStarted: () => awaitThreadStarted(threadId),
      settlement: { kind: 'first-send', mode, engineDraftAtSend },
    });
    if (outcome === 'dropped') {
      // `sendMessage` already toasted, and the draft is all that holds the text.
      rollBack();
      return false;
    }
    if (outcome === 'sent') settleAcceptedFirstSend(threadId);
    // An unsent send consumed nothing, but the engine still holds the pre-send
    // draft, and its unsent message is kept on this device. So write what the
    // composer shows. The picks stay, for a Retry the engine refuses.
    else pushClearedComposeAfterSend(threadId);
    return true;
  } catch (err) {
    rollBack();
    throw err;
  }
}

/** Send a follow-up message in an already-active thread. Clears the local draft
 *  optimistically. The input handler clears the textarea synchronously. Without
 *  a paired clear of the draft signal, the textarea's useEffect resyncs the
 *  typed text on the next render. The Discard draft button then stays visible.
 *  Mirrors the optimistic clear sendCompose applies on promotion. */
export async function sendFollowup(
  threadId: string,
  text: string,
  imageHashes?: string[],
  opts?: { useCodingAgent?: boolean; context?: ChatContext | null; focus?: boolean },
): Promise<void> {
  // Must run before the draft clear — see `markHashesAsSent`.
  if (imageHashes?.length) markHashesAsSent(imageHashes);
  // Clears the draft locally AND schedules the write the send owes the engine.
  // `updateCompose` is the entry point that does both, so this path reaches
  // `pushClearedComposeAfterSend`'s guarantee through it rather than by
  // scheduling a second write.
  updateCompose(threadId, { text: '', image_hashes: [] });
  const outcome = await sendMessage(text, imageHashes, {
    ...opts,
    threadId,
    focus: opts?.focus ?? true,
  });
  // The refused send's row is gone, so the draft is all that can hold the text.
  if (outcome === 'dropped') appendMessagesToCompose(threadId, [{ text, imageHashes: imageHashes ?? [] }]);
}

/** Tab-close-safe flush. Each pending PUT goes out with `keepalive: true`, so
 *  the browser keeps the connection alive after the document tears down.
 *
 *  The device-id header names the caller, which every mutating route needs:
 *  the request carries no other credential here. */
function flushAllPending(): void {
  const headers: Record<string, string> = { 'Content-Type': 'application/json', ...deviceIdHeader() };

  // Every thread holding an intent the engine has not seen. **Two states
  // qualify, not one**: a debounce still counting down (`pendingTimers`), and
  // an intent whose debounce fired but which is QUEUED behind a running write
  // (`owedComposePushes`). The queued one has no timer. Flushing only the
  // timers would drop the newest text on exactly the link that produces it: a
  // write hanging on a stalled connection while the user keeps typing, then the
  // page closing or iOS suspending it.
  const owed = new Set([...pendingTimers.keys(), ...owedComposePushes]);
  for (const timer of pendingTimers.values()) clearTimeout(timer);
  for (const threadId of owed) {
    const thread = threadMap.value.get(threadId);
    if (!thread) continue;
    const draft = getDraft(threadId);
    // Include the per-draft selection, so a dropdown pick made inside the
    // debounce window right before tab close is not lost. Omit when empty, so
    // the backend COALESCE preserves the stored value.
    const selectionOverride = getComposeSelectionOverride(threadId);
    const selectionForFlush = Object.keys(selectionOverride).length > 0 ? selectionOverride : undefined;
    // Always emit the full array on tab close: hashes are tiny. Fenced like
    // every other write. The page is unloading, so this is the last thing we
    // can say. If a submission has since consumed the slot, the engine refusing
    // it is exactly right.
    // Same mode rule as `pushNow`, and this records nothing: the page is
    // unloading, so no ack is observable.
    const body = JSON.stringify({
      text: draft.text,
      image_hashes: draft.image_hashes,
      mode: composeModeForWrite(threadId, thread.meta.state, draft),
      selection: selectionForFlush,
      compose_epoch: knownComposeEpoch(threadId),
    });
    if (body.length > 64 * 1024) {
      // Telemetry carve-out (.claude/rules/frontend.md): the tab is unloading,
      // so any toast would never render. The next foreground page load re-PUTs
      // this draft via the normal debounce path. No data is lost, the draft
      // still being in `composeDrafts` and retried on the next keystroke.
      console.warn(`[compose] keepalive body exceeds 64KB (${body.length}B) for ${threadId}; will retry on next foreground push`);
      continue;
    }
    // `API` carries the gateway base prefix (`/<slug>/api/v1`). A bare
    // `/api/v1/...` would make the gateway read `api` as a workspace slug and
    // 404, dropping the tab-close flush for every gateway-served workspace.
    fetch(`${API}/threads/${encodeURIComponent(threadId)}/compose`, {
      method: 'PUT',
      headers,
      body,
      keepalive: true,
    }).catch(() => { /* tab is unloading */ });
  }
  pendingTimers.clear();
  owedComposePushes.clear();
  // No answer to a keepalive write is observable, so its edits stay pending.
  // An iOS bfcache resume then re-sends them through the ordinary path, whose
  // answer drops the intent.
  for (const threadId of owed) undeliveredComposeDrafts.add(threadId);
}

/** Test-only: run the tab-close flush. */
export function _flushAllPendingForTesting(): void {
  flushAllPending();
}

if (typeof window !== 'undefined') {
  window.addEventListener('beforeunload', flushAllPending);
  window.addEventListener('pagehide', flushAllPending);
}
