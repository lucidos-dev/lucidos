import { showToast, showConfirm, threadMap, archivingThreadIds, applyingNowThreadIds, discardingCCThreadIds, revealOnFocus, resetCodingAgentPendingPreferences, setFocusedThread, focusedThreadId, bootstrappingThreadId, threadChannelFilter, selectedTriggerIds, selectedRepoIds, selectedAppIds, drawerView, threadSearchQuery, threadSearchResults } from '../store';
import { revealThreadPane } from './pane';
import type { ThreadSection, ThreadState } from '../thread-events';
import { threadPassesChannelFilter } from '../threadFilter';
import { computeFamilyGraph, filterByTopThread, orderedCurrentForReview, attentionThreads, reviewThreads, runningThreads, draftThreads } from '../../components/drawer/family-graph';
import type { FamilyGraph } from '../../components/drawer/family-graph';
import { saveThread, unsaveThread, archiveThread } from '../../api/threads';
import { ApiError, putComposeOnThread } from '../../api/client';
import { loadThreadEvents, ensureThreadByIdInMap, sectionMutatedAt } from './thread-loading';
import { clearDraft, draftPresentThreadIds, getDraft, setDraft, type ComposeDraft } from '../composeDrafts';
import { scrollToBottom, scrollToEventAndPulse, scrollToChangeAndPulse, clearPendingEventScroll } from '../../components/chat/scrollState';
import { pushThreadNavState } from './thread-navigation';
import { hasSavedScroll, threadScrollKey } from '../../hooks/useScrollMemory';
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
  setFocusedThread(threadId);
  resetCodingAgentPendingPreferences();
  // Scroll to bottom and suppress ResizeObserver so content rendering
  // doesn't set scrolledUp=true before useAutoScroll can scroll down.
  // Skip when the target has a saved scroll — the 500ms pinning loop
  // would override useScrollMemory's restore.
  // Skip too when a deep-link target (event or change) is provided — the
  // matching scrollTo*AndPulse owns the scroll target in that case.
  const targetEventId = options?.targetEventId ?? null;
  // targetEventId wins when both are set (notification deep-link is more
  // specific than a Changes-row landing on the originating turn).
  const targetChangeId = targetEventId ? null : (options?.targetChangeId ?? null);
  const hasTarget = !!targetEventId || !!targetChangeId;
  // A plain focus (no deep-link target) cancels any in-flight deep-link scroll
  // claim from a prior focus, so its suppression can't leak onto this thread's
  // load. A deep-link focus re-claims below via scrollTo*AndPulse.
  if (!hasTarget) clearPendingEventScroll();
  if (!hasTarget && !hasSavedScroll(threadScrollKey(threadId))) scrollToBottom();
  // notAtTop is NOT reset here — syncNotAtTop() in the scroll listener owns
  // it exclusively. Manual resets cause the chevron to vanish when no scroll
  // event fires (e.g. re-focusing the same thread where scrollTop is unchanged).

  // Lazy-load events for this thread if not already loaded
  loadThreadEvents(threadId);

  pushThreadNavState({ type: 'thread', id: threadId });

  // Surface the focused thread on the pane the user is actually working in:
  // mobile swipes to the thread pane, desktop re-activates the Threads pane
  // group from the cross-group case. Without this, callers like toast onClick
  // and search would set the focused thread but leave the user on whichever
  // pane they were on. See `revealThreadPane` (the mirror of revealContentPane).
  // `revealPane: false` opts out for a bookkeeping focus change (see the option).
  if (options?.revealPane !== false) revealThreadPane();

  if (targetEventId) {
    scrollToEventAndPulse(targetEventId);
  } else if (targetChangeId) {
    scrollToChangeAndPulse(targetChangeId);
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
  const filter = threadChannelFilter.value;
  const triggerSelection = selectedTriggerIds.value;
  const repoSelection = selectedRepoIds.value;
  const appSelection = selectedAppIds.value;
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

  // Pin to bottom and show header before banner re-renders
  scrollToBottom();

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
