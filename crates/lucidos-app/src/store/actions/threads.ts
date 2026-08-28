import { showToast, showConfirm, threadMap, archivingThreadIds, applyingNowThreadIds, discardingCCThreadIds, revealOnFocus, resetCodingAgentPendingPreferences, setFocusedThread, focusedThreadId, bootstrappingThreadId, drawerView, threadSearchQuery, threadSearchResults } from '../store';
import { appliedThreadFilter } from '../appliedThreadFilter';
import { revealThreadPane } from './pane';
import type { ThreadSection, ThreadState } from '../thread-events';
import { describeWaitSubscription } from '../thread-events';
import type { ConfirmDetailGroup, ConfirmDetails } from '../types';
import { threadPassesChannelFilter } from '../threadFilter';
import { computeFamilyGraph, filterByTopThread, orderedCurrentForReview, attentionThreads, reviewThreads, runningThreads, draftThreads } from '../../components/drawer/family-graph';
import type { FamilyGraph } from '../../components/drawer/family-graph';
import { saveThread, unsaveThread, archiveThread } from '../../api/threads';
import { ApiError, putComposeOnThread } from '../../api/client';
import { loadThreadEvents, ensureThreadByIdInMap, refreshStaleThreadEvents, sectionMutatedAt, threadEventsStillArriving } from './thread-loading';
import { clearDraft, draftPresentThreadIds, getDraft, setDraft, type ComposeDraft } from '../composeDrafts';
import { scrollToEventAndPulse, scrollToChangeAndPulse, clearPendingEventScroll, stopFollowingBottom } from '../../components/chat/scrollState';
import { pushThreadNavState } from './thread-navigation';
import { errorDetail } from '../../utils/errorDetail';

// ---------------------------------------------------------------------------
// Thread CRUD
// ---------------------------------------------------------------------------

export interface FocusThreadOptions {
  /** When set, after the thread loads, scroll the matching event card into
   *  view and briefly pulse it. Used by notification deep-links so a push
   *  for a `UserQuestionAsked` lands on that exact question, not the bottom
   *  of the thread or the user's last saved scroll. Overrides the default
   *  scroll-to-bottom / restore-saved-scroll behavior. */
  targetEventId?: string | null;
  /** When set, after the thread loads, scroll to the turn that produced this
   *  change (stamped with `data-change-id` by `ChatExchange`) and pulse it.
   *  Used by the Changes panel so a row lands on its own diff event rather than
   *  the bottom of the thread — the change isn't necessarily the last turn.
   *  Same scroll/suppression contract as `targetEventId`; ignored when
   *  `targetEventId` is also set. */
  targetChangeId?: string | null;
  /** Default true: focusing a thread is navigation, so it surfaces the thread
   *  pane (`revealThreadPane`: mobile swipes, desktop re-activates the Threads
   *  pane group). Pass `false` for a focus change that is BOOKKEEPING rather
   *  than navigation, i.e. the post-archive hand-off in `handleArchiveThread`.
   *  There the focus moves to the next row so the thread pane isn't left
   *  pointing at an archived thread, but the user never asked to go there, and
   *  on mobile the thread drawer IS a pane, so revealing would swipe them off
   *  the list they're triaging. Mirrors `unfocusThread({ revealPane: false })`. */
  revealPane?: boolean;
}

export function focusThread(threadId: string, options?: FocusThreadOptions): void {
  const wasFocused = focusedThreadId.value === threadId;
  setFocusedThread(threadId);
  resetCodingAgentPendingPreferences();
  // Focusing a thread does NOT position its transcript. `useScrollMemory` owns
  // that: a saved position is restored, and a thread with none opens at the top
  // of what is rendered (see ThreadView's `resetOnEmpty`). A deep-link target
  // (event or change) overrides both, via the matching scrollTo*AndPulse below.
  const targetEventId = options?.targetEventId ?? null;
  // targetEventId wins when both are set (notification deep-link is more
  // specific than a Changes-row landing on the originating turn).
  const targetChangeId = targetEventId ? null : (options?.targetChangeId ?? null);
  const hasTarget = !!targetEventId || !!targetChangeId;
  // A plain focus (no deep-link target) cancels any in-flight deep-link scroll
  // claim from a prior focus, so its suppression can't leak onto this thread's
  // load. A deep-link focus re-claims below via scrollTo*AndPulse.
  if (!hasTarget) clearPendingEventScroll();
  // A standing follow belongs to the thread it was armed in. Opening a DIFFERENT
  // thread is not asking to ride its live edge, and the position this one
  // restores to may BE its bottom, which writes no scroll for the reader-moved
  // disarm to notice. Retire it here instead.
  //
  // This costs the thread being LEFT nothing: its request was recorded as the
  // live-edge form of its reading position while it was on screen, and only the
  // ARM is broadcast to the recording side, so a retire writes nothing (see
  // `onFollowArmed`). Re-entry resumes it.
  //
  // Re-focusing the thread the reader is ALREADY in is not an open at all, so it
  // retires nothing: there is no incoming thread to protect, the reader asked for
  // nothing, and `useScrollMemory` does not re-run on an unchanged key, so a
  // retire here would silently end a follow with nothing left to resume it.
  //
  // The one caller that reaches here with the focus ALREADY moved is
  // `focusThreadOrBootstrapResult`'s miss path, which focuses optimistically
  // before its fetch. It retires at that optimistic focus instead, which is the
  // moment its navigation actually leaves a thread.
  if (!wasFocused) stopFollowingBottom();
  // notAtTop is NOT reset here — syncNotAtTop() in the scroll listener owns
  // it exclusively. Manual resets cause the chevron to vanish when no scroll
  // event fires (e.g. re-focusing the same thread where scrollTop is unchanged).

  // Lazy-load events for this thread if not already loaded
  loadThreadEvents(threadId);
  // And catch it up if it IS loaded but a sync point (an iOS PWA wake, an SSE
  // reopen, a `Lagged`) marked it as possibly behind. Those no longer fetch every
  // loaded thread; they mark, and this is where the mark is paid, on the one
  // thread the user is actually opening. No-op for a thread with no mark.
  refreshStaleThreadEvents(threadId);

  pushThreadNavState({ type: 'thread', id: threadId });

  // Surface the focused thread on the pane the user is actually working in:
  // mobile swipes to the thread pane, desktop re-activates the Threads pane
  // group from the cross-group case. Without this, callers like toast onClick
  // and search would set the focused thread but leave the user on whichever
  // pane they were on. See `revealThreadPane` (the mirror of revealContentPane).
  // `revealPane: false` opts out for a bookkeeping focus change (see the option).
  if (options?.revealPane !== false) revealThreadPane();

  // A deep-link whose target never renders used to end in silence, so the tap
  // just looked broken. Either the event is not in this thread, or it renders
  // nothing. scrollState calls back here for the words, staying free of the
  // `store` import that `showToast` would drag in.
  //
  // The message deliberately does NOT claim where the user was taken, because
  // they are not taken anywhere: the transcript stays exactly where it was, and
  // the toast is the whole recovery. It does not name the SOURCE either: a notification tap is no longer the only way in, since the event-wait
  // card's "show it" (`showEventWhereItLives`) lands here too, and telling that
  // user about a notification they never received would be a plain lie.
  //
  // A THIRD case is not a failure and no longer reports: this thread's events
  // were still arriving. The message is a VERDICT about what the thread holds.
  // The calls above started that load, so the deadline used to race a fetch
  // this function had just issued. See `DeepLinkOptions.stillArriving`.
  const stillArriving = () => threadEventsStillArriving(threadId);
  if (targetEventId) {
    scrollToEventAndPulse(targetEventId, {
      stillArriving,
      onUnresolved: () => showToast(
        'That event is not shown in this thread.',
        'warning',
      ),
    });
  } else if (targetChangeId) {
    scrollToChangeAndPulse(targetChangeId, {
      stillArriving,
      onUnresolved: () => showToast(
        'That change is not shown in this thread.',
        'warning',
      ),
    });
  }

  // No auto-read — user must explicitly click Archive, Apply, or Discard.
}

/** Why a bootstrap-and-focus attempt ended. The distinction is load-bearing for
 *  the cross-workspace `#thread=` landing (see `hash-deeplink-router`): a
 *  `not-found` is a verdict from the engine and must not be retried, while a
 *  `failed` is a transport / server error that a peer engine still lazy-starting
 *  behind the gateway routinely produces on the first request. */
export type FocusBootstrapOutcome =
  | { kind: 'focused' }
  | { kind: 'not-found' }
  | { kind: 'failed'; error: unknown };

/** Focus a thread by id, fetching its metadata first if it's not already in
 *  the loaded list (e.g. an old archived thread beyond the Archive per-source
 *  window, or a thread reached via cross-workspace deep link), and report how it
 *  went. Toast-free by design: the caller owns the user-facing message, because
 *  a caller holding durable navigation state (the landing hash) wants to retry a
 *  `failed` before saying anything.
 *
 *  Focuses SYNCHRONOUSLY when the thread is already in the map (that branch runs
 *  before the first `await`), so a caller that only cares about the common case
 *  can ignore the promise without a behavior change. */
export async function focusThreadOrBootstrapResult(
  threadId: string,
  options?: FocusThreadOptions,
): Promise<FocusBootstrapOutcome> {
  if (threadMap.value.has(threadId)) {
    focusThread(threadId, options);
    return { kind: 'focused' };
  }
  // Miss path: a round-trip stands between the tap and anything on screen, and
  // this is where a notification navigating to a thread outside the loaded
  // window lands (always, on a cold push tap: the deep link dispatches while
  // `loadAllThreads` is still in flight, so the map is empty). Acknowledge the
  // tap NOW rather than after the fetch. Focusing optimistically moves the pane
  // and hands `ThreadView` a focused-but-absent thread, which it already renders
  // as its delay-gated skeleton with the 8s "tap to reload" escape hatch. The
  // `bootstrappingThreadId` signal is what stops ThreadView's stale-pointer
  // cleanup from immediately unfocusing it again.
  const previousFocus = focusedThreadId.value;
  bootstrappingThreadId.value = threadId;
  setFocusedThread(threadId);
  // This optimistic focus IS the navigation away from the previous thread, so
  // the standing follow retires HERE. The `focusThread` at the end cannot do it:
  // by then this thread is already the focused one, so its same-thread gate
  // reads "nothing was left" and would keep the PREVIOUS thread's follow armed
  // over the one being opened, which would then ride a live edge nobody asked
  // for and record that borrowed request as its own reading position.
  if (previousFocus !== threadId) stopFollowingBottom();
  // Same opt-out as the hit path above, so `revealPane: false` means the same
  // thing whichever branch a caller lands on.
  if (options?.revealPane !== false) revealThreadPane();
  let found: boolean;
  try {
    found = await ensureThreadByIdInMap(threadId);
  } catch (error) {
    releaseBootstrap(threadId, previousFocus);
    return { kind: 'failed', error };
  }
  if (!found) {
    releaseBootstrap(threadId, previousFocus);
    return { kind: 'not-found' };
  }
  // Clear BEFORE focusing: the thread is in the map now, so ThreadView needs no
  // exemption, and leaving it set would exempt a genuinely stale pointer later.
  if (bootstrappingThreadId.value === threadId) bootstrappingThreadId.value = null;
  focusThread(threadId, options);
  return { kind: 'focused' };
}

/** Undo an optimistic bootstrap focus that didn't land, so the user isn't left
 *  staring at a skeleton for a thread that will never arrive.
 *
 *  A no-op when a NEWER bootstrap has claimed the slot (the user tapped a second
 *  notification mid-flight): that one owns the focus now, and restoring this
 *  call's `previousFocus` would yank them off it.
 *
 *  Each call releases, including the ones inside `landThreadHash`'s retry ladder
 *  (`hash-deeplink-router.ts`), so a cross-workspace landing that loses the race
 *  against a lazy-starting peer engine shows the target's skeleton, drops back to
 *  the prior thread for the backoff, then re-focuses on the next attempt. That
 *  blip is deliberate: holding the focus across attempts instead would mean this
 *  function could no longer release unconditionally, and a caller that forgot to
 *  would leave a thread permanently exempt from ThreadView's stale-pointer
 *  cleanup. Re-capturing `previousFocus` per attempt keeps the restore correct
 *  either way. */
function releaseBootstrap(threadId: string, previousFocus: string | null): void {
  if (bootstrappingThreadId.value !== threadId) return;
  bootstrappingThreadId.value = null;
  if (focusedThreadId.value === threadId) setFocusedThread(previousFocus);
}

/** Fire-and-forget {@link focusThreadOrBootstrapResult} that surfaces the
 *  failure itself. The entry point for every caller with no retry state of its
 *  own (thread-link clicks, notification taps, search results). */
export function focusThreadOrBootstrap(threadId: string, options?: FocusThreadOptions): void {
  void focusThreadOrBootstrapResult(threadId, options).then(outcome => {
    if (outcome.kind === 'not-found') showToast('Thread not found', 'error');
    else if (outcome.kind === 'failed') {
      showToast(`Failed to open thread: ${errorDetail(outcome.error)}`, 'error');
    }
  }).catch(err => {
    // `focusThreadOrBootstrapResult` converts a fetch failure into a `failed`
    // outcome, so reaching here means `focusThread` itself threw. Surface it
    // rather than leave an unhandled rejection (frontend.md, no hidden errors).
    showToast(`Failed to open thread: ${errorDetail(err)}`, 'error');
  });
}

/** Drop the focused thread → the thread pane shows the compose view.
 *
 *  `revealPane` (default true): also surface the thread pane, so the user-intent
 *  callers (the New-thread buttons, the new-chat shortcut, a new-chat
 *  NavigationRequested) land the compose view on the pane the user is looking
 *  at: mobile swipes to it, desktop re-activates the Threads pane group from the
 *  content group. Mirrors focusThread; the callers that used to hand-pair
 *  `navigateToPane('thread')` no longer need to.
 *
 *  Two callers pass `{ revealPane: false }`, both because the unfocus is not
 *  navigation and must not move the visible pane:
 *
 *  - Stale-pointer CLEANUP: ThreadView clears a focusedThreadId whose thread
 *    isn't in the map. ThreadView is mounted in the background on mobile
 *    (MobileSwipeContainer mounts all three panes), so a reveal there would yank
 *    a user on the content pane to the thread pane during render.
 *  - The post-archive hand-off when the last review is dismissed
 *    (`handleArchiveThread`), which must leave a user archiving from the thread
 *    drawer there. See `FocusThreadOptions.revealPane`. */
export function unfocusThread(opts?: { revealPane?: boolean }): void {
  setFocusedThread(null);
  revealOnFocus.value = false;
  resetCodingAgentPendingPreferences();
  // Same rule as `focusThread`'s retire, for the surface it forgot: the compose
  // view has its own scroll container and registers itself as the active one
  // (`CreateThreadView`'s `useScrollObservers`), so a follow armed in a thread
  // would ride the compose view's growth instead. Nothing is lost by retiring,
  // since the thread's own request is recorded under its reading position.
  stopFollowingBottom();
  if (opts?.revealPane !== false) revealThreadPane();
}

// ---------------------------------------------------------------------------
// Save / Unsave
// ---------------------------------------------------------------------------
// Save is offered on Review/Archive sections at idle. Unsave is offered on
// the Saved section mid-turn — the only way to drop a running thread out of
// Saved without canceling it. Confirm before unsave so a stray click doesn't
// cost the parking spot.

/** Translate a failed `archiveThread` call into a user-facing toast string.
 *  The engine returns a structured 409 body (`reason`, `parent_status`,
 *  `blocking`) for the cascade-gate rejections — without the formatter the
 *  toast falls back to `"409"` (empty `statusText`, no `body.error`), which
 *  tells the user nothing actionable. */
function formatArchiveErrorToast(err: unknown): string {
  if (err instanceof ApiError && err.httpCode === 409 && err.body && typeof err.body === 'object') {
    const body = err.body as Record<string, unknown>;
    if (body.reason === 'descendants_blocking') {
      const blocking = Array.isArray(body.blocking) ? body.blocking : [];
      const n = blocking.length;
      if (n === 1) return "Can't archive yet — a sub-thread is still busy";
      if (n > 1) return `Can't archive yet — ${n} sub-threads are still busy`;
      return "Can't archive yet — a sub-thread is still busy";
    }
    if (body.reason === 'parent_not_archivable') {
      // Archive is idempotent: an already-archived target is a no-op success
      // (200), not a 409, so `parent_not_archivable` is now raised ONLY for
      // live work (status === 'running'). See `classify_archive_decision` in
      // crates/lucidos-engine/src/api/threads/archive.rs.
      return "Can't archive yet — this thread is still running";
    }
    if (body.reason === 'parent_has_pending_changes') {
      return "Can't archive — apply or discard the pending change first";
    }
  }
  return `Failed to archive thread: ${errorDetail(err)}`;
}

function updateThreadMeta(threadId: string, patch: Partial<{ saved: boolean }>): void {
  const map = new Map(threadMap.value);
  const thread = map.get(threadId);
  if (thread) {
    map.set(threadId, { ...thread, meta: { ...thread.meta, ...patch } });
    threadMap.value = map;
  }
}

export async function handleSaveThread(threadId: string): Promise<void> {
  const thread = threadMap.value.get(threadId);
  if (!thread || thread.meta.saved) return;

  updateThreadMeta(threadId, { saved: true });
  try {
    await saveThread(threadId);
  } catch (e) {
    // A 409 means the thread is already saved — the desired end-state already
    // holds (a racing/duplicate submit, or a stale client hitting an older
    // engine). Keep the optimistic pin and stay quiet; only real failures
    // (network, 5xx) revert + toast. The server is idempotent now, so a fresh
    // engine won't even 409 here — this is defense in depth.
    if (e instanceof ApiError && e.httpCode === 409) return;
    updateThreadMeta(threadId, { saved: false });
    showToast(`Failed to pin thread: ${errorDetail(e)}`, 'error');
  }
}

export async function handleUnsaveThread(threadId: string): Promise<void> {
  const thread = threadMap.value.get(threadId);
  if (!thread || !thread.meta.saved) return;

  if (!await showConfirm('Remove this thread from the Pinned section?', 'Remove')) {
    return;
  }

  updateThreadMeta(threadId, { saved: false });
  try {
    await unsaveThread(threadId);
  } catch (e) {
    // Mirror of the save path: a 409 means the thread is already unsaved, so
    // the desired end-state holds — keep the optimistic unpin and stay quiet.
    if (e instanceof ApiError && e.httpCode === 409) return;
    updateThreadMeta(threadId, { saved: true });
    showToast(`Failed to unpin thread: ${errorDetail(e)}`, 'error');
  }
}

// ---------------------------------------------------------------------------
// Archive (move waiting thread to archive)
// ---------------------------------------------------------------------------

/** Threads the user can actually see, given the active channel/trigger/repo/app
 *  filter — the SAME family-scoped predicate the drawer uses (`ThreadDrawer`'s
 *  ThreadList). The post-archive focus must land on a thread the user can click
 *  on in the list, so a thread hidden by the active filter is never offered as
 *  the next focus. Returns the filtered set plus the family graph (built over
 *  the full thread map so parent walks resolve) for the caller to order. */
function visibleThreadsAndGraph(): { visible: ThreadState[]; graph: FamilyGraph } {
  const all = Array.from(threadMap.value.values());
  const graph = computeFamilyGraph(all);
  // The *applied* selection, which is what the drawer is showing: while the
  // thread filter panel is up it holds still, and the row the focus lands on
  // has to be one the user can actually click (see `appliedThreadFilter`).
  const { channels: filter, triggerIds: triggerSelection, repoIds: repoSelection, appIds: appSelection } = appliedThreadFilter.value;
  // A trigger/repo/app sub-selection flips the gate to any-member matching,
  // mirroring the drawer so a coding-agent thread in the selected repo/app surfaces with
  // its family even when the root's channel is filtered out.
  const subSelectionActive =
    triggerSelection.size > 0 || repoSelection.size > 0 || appSelection.size > 0;
  const visible = filterByTopThread(all, graph,
    t => threadPassesChannelFilter(t, filter, triggerSelection, repoSelection, appSelection),
    subSelectionActive,
  );
  return { visible, graph };
}

/** The visible thread ids of the drawer view the user is *currently looking at*,
 *  in the same order the drawer renders them. The post-archive focus walks this
 *  so "next" is the next visible row in whatever view is active — the next
 *  Needs-attention row when the attention view is open, the next Review/Running
 *  row, the next filtered Current row in the default view — instead of always
 *  jumping into Current. Mirrors `ThreadDrawer`'s `activeView` resolution: a live
 *  search query overrides the selected view; otherwise `drawerView` decides.
 *
 *  The alternate views (attention/review/running/drafts) deliberately bypass the
 *  channel/trigger/repo/app filter — exactly as the drawer renders them — so the
 *  next focus matches what's on screen. Only the default `all` view is
 *  filter-aware, walking the visible Current section (`orderedCurrentForReview`)
 *  the same way the drawer does. */
function orderedVisibleThreadIds(): string[] {
  // Search overrides the selected view (mirrors ThreadDrawer's activeView).
  if (threadSearchQuery.value.trim().length > 0) {
    const results = threadSearchResults.value;
    return results.status === 'loaded' ? results.data.map(r => r.thread_id) : [];
  }
  const map = threadMap.value;
  switch (drawerView.value) {
    case 'attention': return attentionThreads(map).map(t => t.meta.id);
    case 'review':    return reviewThreads(map).map(t => t.meta.id);
    case 'running':   return runningThreads(map).map(t => t.meta.id);
    case 'drafts':    return draftThreads(map).map(t => t.meta.id);
    case 'all':
    default: {
      const { visible, graph } = visibleThreadsAndGraph();
      return orderedCurrentForReview(visible, graph).map(t => t.meta.id);
    }
  }
}

/** Ordered list of visible thread ids to consider as the next focus when the
 *  user archives `aroundId` — closest below first, then closest above — within
 *  the currently active drawer view (`orderedVisibleThreadIds`). Snapshotted
 *  BEFORE the optimistic flip so the position anchor survives the cascade
 *  dropping `aroundId` (and its descendants) out of the view. */
function visibleCandidatesAround(aroundId: string): string[] {
  const ordered = orderedVisibleThreadIds();
  const idx = ordered.indexOf(aroundId);
  if (idx < 0) return [];
  const result: string[] = [];
  for (let i = idx + 1; i < ordered.length; i++) result.push(ordered[i]);
  for (let i = idx - 1; i >= 0; i--) result.push(ordered[i]);
  return result;
}

/** Walk parentThreadId from every thread in the map to collect the target +
 *  every transitive descendant. Mirrors the backend cascade scope so the
 *  optimistic flip drops the whole family out of review in one stroke. */
function collectArchiveCascade(rootId: string): Set<string> {
  const childrenByParent = new Map<string, string[]>();
  for (const t of threadMap.value.values()) {
    const p = t.meta.parentThreadId;
    if (!p) continue;
    const bucket = childrenByParent.get(p);
    if (bucket) bucket.push(t.meta.id); else childrenByParent.set(p, [t.meta.id]);
  }
  const seen = new Set<string>();
  const stack: string[] = [rootId];
  while (stack.length > 0) {
    const id = stack.pop()!;
    if (seen.has(id)) continue;
    seen.add(id);
    const kids = childrenByParent.get(id);
    if (kids) stack.push(...kids);
  }
  return seen;
}

/** What archiving this cascade would stop, for the confirm to name.
 *
 *  Archiving cancels every live *thread subscription* in the cascade, which is
 *  correct and stays: leaving one live behind the archive curtain would wake a
 *  thread the user considers closed. The bug was that it happened in silence,
 *  so an ordinary unsaved thread with three live subscriptions archived on the
 *  first tap and the event-wait dispatcher cancelled all three.
 *
 *  **Two sources, because a row can be counted before it is named.** Both now
 *  come from the same projection: `meta.liveEventWaitCount` and
 *  `meta.liveEventWaits` arrive together on every thread summary, so a row in
 *  the map normally names every subscription it holds. They can still differ
 *  for a row assembled from another path, an optimistic SSE skeleton or a
 *  fixture that carries the count alone. So the named ones come from the list,
 *  and any remainder is counted from the column. The dialog never waits on a
 *  fetch before it can open.
 *
 *  Returns `null` when the cascade holds none, which is what keeps an ordinary
 *  archive a single tap.
 *
 *  Exported for its own tests: the naming and the remainder line are worth
 *  pinning without driving a whole archive. */
export function subscriptionsStoppedByArchive(
  cascade: Set<string>,
  rootId: string,
): { message: string; details: ConfirmDetails } | null {
  const groups: ConfirmDetailGroup[] = [];
  let named = 0;
  let unnamed = 0;
  for (const id of cascade) {
    const t = threadMap.value.get(id);
    if (t === undefined || t.meta.liveEventWaitCount === 0) continue;
    const waits = t.meta.liveEventWaits;
    if (waits.length > 0) {
      named += waits.length;
      groups.push({
        header: id === rootId ? 'This thread' : t.meta.title || 'Sub-thread',
        items: waits.map((w) => `${w.reason} (${describeWaitSubscription(w.on)})`),
      });
    }
    // The shortfall is counted whether the thread named NONE of its
    // subscriptions or only some. Handling only the all-or-nothing case would
    // name one and drop the other, under-reporting the total in the one dialog
    // whose whole job is not to.
    unnamed += Math.max(0, t.meta.liveEventWaitCount - waits.length);
  }
  const count = named + unnamed;
  if (count === 0) return null;
  // A count-only group: the dialog renders a header with no list, which is the
  // honest shape for subscriptions we can count but not name.
  if (unnamed > 0) {
    groups.push({
      header: `${unnamed} more on sub-threads`,
      items: [],
    });
  }
  return {
    message:
      count === 1
        ? 'Archiving stops what this thread is waiting for. It will not fire.'
        : `Archiving stops ${count} subscriptions. They will not fire.`,
    details: { groups },
  };
}

/** Clear a thread's unsent reply draft — local signal plus the server compose
 *  row. Snapshots the draft so a failed PUT restores it: local and server must
 *  not diverge, or the discarded draft silently reappears on the next load.
 *  Deliberately uses the leaf `composeDrafts` helpers + `putComposeOnThread`
 *  rather than compose.ts's `updateCompose`, so core thread CRUD doesn't pull
 *  the chat-send graph (compose.ts → chat.ts) into its imports. */
function discardThreadDraft(threadId: string): void {
  const prior = getDraft(threadId);
  const restore: ComposeDraft = { ...prior, image_hashes: [...prior.image_hashes] };
  clearDraft(threadId);
  void putComposeOnThread(threadId, '', [], null).catch((e) => {
    setDraft(threadId, restore);
    showToast(`Couldn't discard draft: ${errorDetail(e)}`, 'error');
  });
}

export async function handleArchiveThread(threadId: string): Promise<void> {
  if (archivingThreadIds.value.has(threadId)) return;
  if (discardingCCThreadIds.value.has(threadId)) return; // Can't archive while discarding

  // Archive is the only exit from Saved — confirm before dropping the row out
  // of its parking spot. The ThreadArchived projection clears is_saved.
  const thread = threadMap.value.get(threadId);
  if (thread?.meta.saved) {
    if (!await showConfirm(
      'Are you sure you want to move this thread to the archive?',
      'Archive',
    )) {
      return;
    }
  }

  // The archive cascades to the target + every transitive descendant; collect
  // the family up front so we can both check it for unsent drafts here and (just
  // below) flip the whole family out of review in one stroke.
  const cascade = collectArchiveCascade(threadId);

  // If any family member carries an unsent reply draft, ask whether to discard
  // it too. Archiving doesn't clear the draft server-side, so the focused OK
  // button ("Keep draft") is the conservative default — it leaves the draft to
  // resume after un-archiving. "Discard draft" (the left extraAction) clears it
  // on every drafted member once the archive succeeds; Cancel/Escape aborts the
  // archive entirely. Mirrors the Apply/Discard/Cancel shape in threadActions.ts
  // (destructive = extraAction, safe = the focused OK button).
  const draftedIds = [...cascade].filter((id) => draftPresentThreadIds.value.has(id));
  let discardDrafts = false;
  if (draftedIds.length > 0) {
    const many = draftedIds.length > 1;
    let discardChosen = false;
    const keep = await showConfirm(
      many
        ? 'These threads have unsent drafts. Discard them too?'
        : 'This thread has an unsent draft. Discard it too?',
      many ? 'Keep drafts' : 'Keep draft',
      {
        variant: 'default',
        cancelLabel: 'Cancel',
        extraAction: {
          label: many ? 'Discard drafts' : 'Discard draft',
          onClick: () => { discardChosen = true; },
        },
      },
    );
    // keep === true → OK ("Keep draft(s)"): archive, leave the draft.
    // discardChosen → extraAction ("Discard draft(s)"): archive + clear below.
    // neither → Cancel/Escape/outside-click: abort the archive.
    if (!keep && !discardChosen) return;
    discardDrafts = discardChosen;
  }

  // Archiving cancels every live thread subscription in the same cascade, so
  // say which ones before it happens. Unlike the draft confirm there is no
  // third outcome to offer: keeping a subscription alive behind the archive
  // curtain is not on the table, so this is Cancel (abort) versus Archive
  // (proceed), and a cascade holding none never asks at all.
  const stopping = subscriptionsStoppedByArchive(cascade, threadId);
  if (stopping) {
    const proceed = await showConfirm(stopping.message, 'Archive', {
      variant: 'default',
      cancelLabel: 'Cancel',
      details: stopping.details,
    });
    if (!proceed) return;
  }

  // Clear stale apply state — applying and archiving are mutually exclusive.
  // If the user is archiving, any in-progress or stale apply is abandoned.
  if (applyingNowThreadIds.value.has(threadId)) {
    const next = new Map(applyingNowThreadIds.value);
    next.delete(threadId);
    applyingNowThreadIds.value = next;
  }

  // Snapshot the position anchor BEFORE the optimistic flip — once the
  // cascade leaves the active view, visibleCandidatesAround() can't compute it.
  const candidates = visibleCandidatesAround(threadId);

  // Snapshot section + codingAgentProposed on every family member so we can
  // roll back if the API rejects (409 blocking, 500 mid-cascade). Both fields
  // are required to leave Current: `displaySection` keeps any thread with
  // pending changes in Current regardless of `section`. `cascade` was collected
  // up front (above the draft confirm).
  type Snap = { section: ThreadSection; codingAgentProposed: boolean };
  const snapshot = new Map<string, Snap>();
  const optimistic = new Map(threadMap.value);
  // Stamp BEFORE the flip so any in-flight GET issued before this moment is
  // considered stale wrt section/codingAgentProposed. See `sectionMutatedAt`
  // in thread-loading.ts for the iOS-PWA-resume race this prevents.
  const flippedAt = Date.now();
  for (const tid of cascade) {
    const t = optimistic.get(tid);
    if (!t) continue;
    sectionMutatedAt.set(tid, flippedAt);
    snapshot.set(tid, {
      section: t.meta.section,
      codingAgentProposed: t.meta.codingAgentProposed,
    });
    optimistic.set(tid, {
      ...t,
      meta: { ...t.meta, section: 'archived', codingAgentProposed: false },
    });
  }
  threadMap.value = optimistic;

  // Every cascade member gets the in-flight flag, not just the root: the
  // backend's stop_agent emits CodingAgentIdled for each descendant with the
  // PRE-archive aggregate (section='inbox'), so the SSE archive-race guard
  // in thread-sync.ts needs to recognise descendants as in-flight too.
  archivingThreadIds.value = new Set([...archivingThreadIds.value, ...cascade]);

  // The SSE `ThreadArchived` cascade arriving later just confirms what we
  // already did.
  //
  // Both branches move the focus WITHOUT revealing the thread pane: archiving
  // is not navigation. The hand-off exists so the thread pane isn't left
  // pointing at a thread that just left the list, not because the user asked to
  // go there. On mobile the thread drawer is its own pane, so revealing swiped a
  // user archiving row after row out of the list on every tap.
  const nextId = candidates.find(id => !cascade.has(id)) ?? null;
  if (nextId) {
    revealOnFocus.value = true;
    focusThread(nextId, { revealPane: false });
  } else {
    // Last review dismissed → the thread pane falls back to the compose view.
    unfocusThread({ revealPane: false });
  }

  try {
    await archiveThread(threadId);
    // Archive landed — now honor a "Discard draft(s)" choice. Deferred until
    // here so a rejected archive (rolled back below) leaves the draft intact.
    if (discardDrafts) {
      for (const id of draftedIds) discardThreadDraft(id);
    }
  } catch (e) {
    const restored = new Map(threadMap.value);
    for (const [tid, snap] of snapshot) {
      const t = restored.get(tid);
      if (!t) continue;
      restored.set(tid, { ...t, meta: { ...t.meta, ...snap } });
    }
    threadMap.value = restored;
    // Re-focus the rejected thread only if the user hasn't actively navigated
    // away during the in-flight API call — a user who picked a different
    // thread made a deliberate choice we shouldn't yank them out of.
    const stillOnAutoFocus = focusedThreadId.value === nextId;
    if (stillOnAutoFocus && restored.has(threadId)) {
      focusThread(threadId, { revealPane: false });
    }
    showToast(formatArchiveErrorToast(e), 'error');
  } finally {
    const next = new Set(archivingThreadIds.value);
    for (const tid of cascade) next.delete(tid);
    archivingThreadIds.value = next;
  }
}
