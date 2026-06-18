/**
 * Server-only compose state. No localStorage.
 *
 * Draft text / images / mode pick live in the sibling `composeDrafts` signal
 * (see `store/composeDrafts.ts`). `threadMap[id].meta.state` is the lifecycle
 * marker (composing → active → discarded → archived); the draft signal moves
 * separately so per-keystroke writes don't ripple through every threadMap
 * subscriber. Mutations go through:
 *   - updateCompose(id, patch)         — optimistic local + debounced PUT
 *   - startComposeIfNeeded(id, mode)   — POST /threads (idempotent)
 *   - discardCompose(id)               — DELETE /threads/:id, state→discarded
 *   - sendCompose(id, text, images, opts) — chat POST + state→active locally
 *   - sendFollowup(id, text, images, opts) — chat POST on already-active thread; clears draft
 *
 * The SSE consumer in thread-sync.ts applies remote ThreadStarted /
 * ThreadDiscarded / ThreadComposeChanged with origin_device_id and
 * focused-textarea guards.
 */

import { threadMap, focusedThreadId, inputMode, showToast, setFocusedThread, selectedScope, selectedCodingAgent, resetCodingAgentPendingPreferences, type Scope } from '../store';
import type { ComposeDestination } from '../composeDestination';
import { dismissComposeHandoffHint } from './preferences';
import { makeOptimisticThreadState, type ThreadMeta } from '../thread-events';
import { clearDraft, composeDrafts, draftIsEmpty, getDraft, patchDraft, setDraft, type ComposeDraft } from '../composeDrafts';
import { ApiError, ensureThreadStarted, putComposeOnThread, deleteThread } from '../../api/client';
import { errorDetail } from '../../utils/errorDetail';
import { sendMessage } from './chat';
import type { ChatContext } from './chatContext';
import { markHashesAsSent } from '../../components/chat/pastedImages';
import { pushThreadNavState, removeThreadNavEntries } from './thread-navigation';

export type ComposeMode = 'lucidos' | 'claude_code';

function scopeEquals(a: Scope, b: Scope): boolean {
  if (a.kind !== b.kind) return false;
  if (a.kind === 'external' && b.kind === 'external') return a.repoId === b.repoId;
  if (a.kind === 'app' && b.kind === 'app') return a.appId === b.appId;
  return true;
}

/** Apply a picked compose destination: set the channel (global `inputMode` +
 *  the composing draft's `mode`, mirroring the retired segmented-control
 *  `setMode`) and, for coding targets, the scope. Each write is guarded so a
 *  same-value re-pick (the Dropdown fires onChange on every click, including
 *  the already-selected option) is a no-op — no signal-identity churn, no
 *  debounced compose PUT, no SSE fan-out. The draft patch only applies to a
 *  focused composing thread — once active, the channel is locked server-side
 *  and the picker is hidden anyway. */
export function applyDestination(threadId: string | null, d: ComposeDestination): void {
  const mode: ComposeMode = d.kind === 'coding' ? 'claude_code' : 'lucidos';
  const modeType = mode === 'claude_code' ? 'coding_agent' : 'do';
  if (inputMode.value.type !== modeType) {
    inputMode.value = mode === 'claude_code' ? { type: 'coding_agent' } : { type: 'do' };
  }
  if (d.kind === 'coding' && !scopeEquals(selectedScope.value, d.scope)) {
    selectedScope.value = d.scope;
  }
  if (!threadId) return;
  const thread = threadMap.value.get(threadId);
  if (thread?.meta.state !== 'composing') return;
  // null draft.mode is a real change: the patch locks the pick server-side.
  if (getDraft(threadId).mode !== mode) {
    updateCompose(threadId, { mode });
  }
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
 *  peer device sees the change within a normal eye-blink; long enough that a
 *  continuous typist doesn't flood the engine + SSE fan-out per character. */
const DEBOUNCE_MS = 250;

const pendingTimers = new Map<string, ReturnType<typeof setTimeout>>();

/** Thread ids with a pending PUT — covers the entire window from "optimistic
 *  local write committed" through "server PUT acked", including the 250ms
 *  debounce. SSE and loadAllThreads consult this set to avoid clobbering a
 *  local change the server hasn't seen yet. The debounce window matters on
 *  iOS PWA: PHPicker dismissal fires visibilitychange (→ loadAllThreads) at
 *  roughly the same instant as the file input's change event, so without
 *  covering the debounce, a freshly attached image is overwritten by the
 *  server's stale empty array before the PUT even goes out. */
export const pendingComposePuts = new Set<string>();

/** Per-thread timestamp of the last local compose mutation (Date.now()).
 *  loadAllThreads / ensureThread* capture their own request start time and
 *  consult this map: if the thread was edited *after* the GET started, the
 *  response is by definition stale wrt compose state and the overwrite is
 *  skipped. Without this, a stale GET issued before the user's photo attach
 *  but whose response lands AFTER pushNow's PUT clears `pendingComposePuts`
 *  silently overwrites the optimistic image with the server's pre-PUT
 *  snapshot — preview appears, then disappears. Stays set forever (no
 *  expiry); cross-device sync still works because legitimate refreshes
 *  capture a request time AFTER the last local edit. */
export const composeEditedAt = new Map<string, number>();

function markLocallyEdited(threadId: string): void {
  composeEditedAt.set(threadId, Date.now());
}

/** Single entry point for compose mutations from the UI. Optimistic local
 *  apply then debounced server PUT. A composing thread that ends up empty
 *  after the patch is auto-discarded — never-sent + no content would
 *  otherwise render as a ghost row titled "Empty draft". updatedAt is
 *  intentionally left alone: typing a draft is not "activity" the drawer
 *  should surface; the backend's last-activity allowlist agrees. */
export function updateCompose(threadId: string, patch: ComposePatch): void {
  markLocallyEdited(threadId);
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

/** Apply an SSE ThreadComposeChanged from a peer device. Caller must check
 *  origin_device_id, pendingComposePuts, and focused-textarea guards before
 *  invoking. Replaces the draft wholesale — SSE carries the full snapshot.
 *  Skips unknown threads so a stray broadcast can't seed an orphan draft
 *  entry; empty payloads clear the entry instead of inflating the Map. */
export function applyRemoteCompose(
  threadId: string,
  fields: { text: string; image_hashes: string[]; mode: ComposeMode | null },
): void {
  if (!threadMap.value.has(threadId)) return;
  if (fields.text === '' && fields.image_hashes.length === 0 && fields.mode === null) {
    clearDraft(threadId);
    return;
  }
  setDraft(threadId, fields);
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

function schedulePush(threadId: string): void {
  const existing = pendingTimers.get(threadId);
  if (existing) clearTimeout(existing);
  // Mark pending before the debounce so concurrent loads don't clobber.
  pendingComposePuts.add(threadId);
  const t = setTimeout(() => {
    pendingTimers.delete(threadId);
    pushNow(threadId).catch((err) => {
      // Surface — the user's local state diverged from the server and we
      // need them to know they're typing into thin air.
      showToast(`Compose sync failed: ${errorDetail(err)}`, 'error');
    });
  }, DEBOUNCE_MS);
  pendingTimers.set(threadId, t);
}

/** When the next push has the same array as the last one we synced, send
 *  `null` so the server's COALESCE preserves and skips the SSE re-broadcast. */
const lastSyncedImageHashes = new Map<string, string[]>();

function imageHashesUnchanged(threadId: string, current: string[]): boolean {
  const prev = lastSyncedImageHashes.get(threadId);
  if (!prev) return false;
  if (prev.length !== current.length) return false;
  return prev.every((h, i) => h === current[i]);
}

async function pushNow(threadId: string): Promise<void> {
  try {
    try {
      // PUT 404s if the 250ms debounce elapses before POST /threads settles.
      await awaitThreadStarted(threadId);
    } catch {
      // ensureFocusedComposeThread already toasts the start failure; a second
      // toast from this PUT path would just duplicate the same error.
      return;
    }
    const thread = threadMap.value.get(threadId);
    // Discarded threads stay in threadMap with state='discarded' until the
    // SSE confirms the DELETE, so existence alone is not enough — a PUT here
    // would 410 against a thread the user already discarded.
    if (!thread || thread.meta.state === 'discarded') return;
    const draft = getDraft(threadId);
    // null = preserve via COALESCE (avoids the SSE re-broadcast).
    const wireHashes: string[] | null = imageHashesUnchanged(threadId, draft.image_hashes)
      ? null
      : [...draft.image_hashes];
    await putComposeOnThread(
      threadId,
      draft.text,
      wireHashes,
      // Mode is only meaningful for composing threads — once active, the
      // channel field is authoritative and the server rejects mode changes.
      thread.meta.state === 'composing' ? draft.mode : null,
    );
    if (wireHashes !== null) {
      lastSyncedImageHashes.set(threadId, wireHashes);
    }
  } finally {
    pendingComposePuts.delete(threadId);
  }
}

function cancelPendingPush(threadId: string): void {
  const t = pendingTimers.get(threadId);
  if (t) {
    clearTimeout(t);
    pendingTimers.delete(threadId);
    pendingComposePuts.delete(threadId);
  }
}

/** Idempotent: POST /threads with `{id, mode}`. No-op if the thread is
 *  already in threadMap (server is also idempotent on the same `{id, mode}`,
 *  but skipping the round-trip on the hot path matters for first-keystroke
 *  latency). Inserts an optimistic composing entry so the UI has somewhere
 *  to write before the server ack lands, plus a draft entry holding the
 *  user's mode pick. */
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
  try {
    await ensureThreadStarted(threadId, mode);
  } catch (err) {
    rollbackOptimistic(threadId);
    throw err;
  }
}

function rollbackOptimistic(threadId: string): void {
  if (!threadMap.value.has(threadId)) return;
  const next = new Map(threadMap.value);
  next.delete(threadId);
  threadMap.value = next;
  clearDraft(threadId);
}

/** In-flight POST /threads promises keyed by thread id. Callers that need
 *  the row to exist server-side before issuing their own request (image
 *  blob upload — only attach path that fires synchronously, no debounce
 *  to hide the race) consult this via `awaitThreadStarted`. */
const pendingThreadStarts = new Map<string, Promise<void>>();

/** Resolve once the in-flight `POST /threads` for this id has settled.
 *  No-op (resolves immediately) if no start is in flight — covers both the
 *  already-active thread case and any later race-free caller. */
export async function awaitThreadStarted(threadId: string): Promise<void> {
  const p = pendingThreadStarts.get(threadId);
  if (p) await p;
}

/** Lazy-create a thread id when the user starts composing without a thread
 *  focused. The id is allocated client-side; `startComposeIfNeeded` POSTs the
 *  row server-side. Toast on POST failure — local optimism gets rolled back
 *  by `startComposeIfNeeded` itself. */
export function ensureFocusedComposeThread(): string {
  let id = focusedThreadId.value;
  if (id) return id;
  id = crypto.randomUUID();
  setFocusedThread(id);
  // Inlined instead of focusThread() — focusThread also fires scrollToBottom,
  // loadThreadEvents, and (on mobile) navigateToPane, none of which the
  // draft path wants.
  pushThreadNavState({ type: 'thread', id });
  const startPromise = startComposeIfNeeded(id, currentComposeMode());
  pendingThreadStarts.set(id, startPromise);
  startPromise
    .catch((err) => {
      // Mirror rollbackOptimistic's threadMap drop in nav so Forward can't later
      // restore an id whose threadMap entry no longer exists.
      removeThreadNavEntries(id);
      if (focusedThreadId.value === id) setFocusedThread(null);
      showToast(`Failed to start compose: ${errorDetail(err)}`, 'error');
    })
    .finally(() => {
      // Only clear if we still own the slot — a fresh ensureFocusedComposeThread
      // for the same id (rare but possible after rollback + reuse) wins.
      if (pendingThreadStarts.get(id) === startPromise) {
        pendingThreadStarts.delete(id);
      }
    });
  return id;
}

/** Prefill the compose input with a starter prompt (the new-workspace
 *  suggestion chips). Lazily focuses/creates a composing thread, then writes
 *  the text through the normal debounced compose path so it syncs to the
 *  textarea and persists like any typed draft. Does NOT send — the user
 *  reviews/edits, picks a destination, and hits Send themselves. Returns the
 *  thread id the text landed on. */
export function prefillCompose(text: string): string {
  const threadId = ensureFocusedComposeThread();
  updateCompose(threadId, { text });
  return threadId;
}

/** Discard a composing thread. Optimistic state→discarded for instant
 *  feedback, then DELETE /threads/:id. 404/410 (already gone server-side)
 *  is the desired end-state — swallow. Other errors roll back and toast.
 *  Releases focus if the discarded thread was the focused one so the next
 *  keystroke lazy-creates a fresh draft via ensureFocusedComposeThread.
 *  The draft entry is dropped; if DELETE fails we restore it to the
 *  pre-discard text so the user doesn't lose what they typed. */
export async function discardCompose(threadId: string): Promise<void> {
  cancelPendingPush(threadId);
  if (focusedThreadId.value === threadId) setFocusedThread(null);
  const restoreDraft = snapshotDraft(threadId);
  mutateThreadMeta(threadId, { state: 'discarded' });
  clearDraft(threadId);
  lastSyncedImageHashes.delete(threadId);
  // Pair with the push in ensureFocusedComposeThread — Back/Forward must not
  // restore a discarded thread whose events would 404.
  removeThreadNavEntries(threadId);
  try {
    await deleteThread(threadId);
  } catch (err) {
    if (isAlreadyGone(err)) return;
    mutateThreadMeta(threadId, { state: 'composing' });
    if (restoreDraft) setDraft(threadId, restoreDraft);
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

/** Send the focused thread's current compose contents as the first message.
 *  Reads text/images from the draft signal so the caller doesn't need to
 *  pass them. Optimistic local clear + state→active before the chat POST so
 *  the input is immediately usable for follow-up text. On failure we restore
 *  the typed text — losing it would be the worst possible UX. */
export async function sendCompose(
  threadId: string,
  opts: { useCodingAgent?: boolean; context?: ChatContext | null; focus?: boolean },
): Promise<void> {
  const thread = threadMap.value.get(threadId);
  if (!thread) return;
  const draft = getDraft(threadId);
  const text = draft.text;
  const wireHashes = draft.image_hashes;
  const mode = draft.mode;
  if (!text.trim() && wireHashes.length === 0) return;

  cancelPendingPush(threadId);
  // Bind here so sendMessage doesn't have to detect first-send vs follow-up
  // (see frontend.md "Drafts Are Threads"). selectedScope may drift afterward.
  // Channel is locked from `opts.useCodingAgent` (which the caller resolved
  // via effectiveSendMode) rather than the existing meta.channel: the latter
  // was stamped at first-keystroke time from `currentComposeMode()` and goes
  // stale the moment the user toggles. Without this lock, sendMessage reads
  // the stale channel, ignores the explicit useCodingAgent option, and routes
  // a coding-agent send through the Lucidos Agent (or vice versa).
  const scope = selectedScope.value;
  const boundRepoId = opts.useCodingAgent && scope.kind === 'external' ? scope.repoId : undefined;
  const boundCodingAgentKind = opts.useCodingAgent ? scope.kind : undefined;
  const boundCodingAgentFolder = opts.useCodingAgent && scope.kind === 'app'
    ? `data/apps/${scope.appId}`
    : undefined;
  const boundChannel: ThreadMeta['channel'] = opts.useCodingAgent ? 'claude_code' : 'chat';
  const boundCodingAgent = opts.useCodingAgent ? selectedCodingAgent.value : undefined;
  mutateThreadMeta(threadId, {
    state: 'active',
    channel: boundChannel,
    repoId: boundRepoId,
    codingAgentKind: boundCodingAgentKind,
    codingAgentFolder: boundCodingAgentFolder,
    codingAgent: boundCodingAgent,
  });
  // Must run before the draft clear — see `markHashesAsSent`.
  if (wireHashes.length > 0) markHashesAsSent(wireHashes);
  clearDraft(threadId);
  lastSyncedImageHashes.delete(threadId);
  const shouldFocus = opts.focus ?? true;
  if (shouldFocus) setFocusedThread(threadId);
  try {
    await sendMessage(text, wireHashes.length > 0 ? wireHashes : undefined, {
      useCodingAgent: opts.useCodingAgent,
      context: opts.context,
      threadId,
      focus: shouldFocus,
    });
    // A compose-view CC model/effort pick is a one-shot intent: it has now been
    // carried into this spawn's chat body (chat.ts reads ccPending* there), so
    // consume it. Without this, the pending pick — a module signal only reset by
    // focusThread/unfocusThread, neither of which the compose→send path hits
    // (setFocusedThread above is the low-level setter) — rides onto every
    // subsequent new thread, overriding the user's ~/.claude effortLevel default
    // with a stale value. Follow-ups (sendFollowup) deliberately don't clear
    // here: their pending is reconciled per-thread by CodingAgentControlMenu.loadCommands.
    resetCodingAgentPendingPreferences();
    // First send to a coding destination retires the compose view's
    // "Not sure? Start with the Lucidos Agent" hint — the user has found the
    // hand-off's explicit sibling on their own. Idempotence lives inside the
    // helper; savePreference surfaces a failed write via toast, so
    // fire-and-forget is safe here.
    if (opts.useCodingAgent) {
      void dismissComposeHandoffHint();
    }
  } catch (err) {
    // Roll back state. Restore text/images only if the user hasn't started
    // typing into the now-empty textarea — overwriting fresh keystrokes
    // would lose work the user can see they typed.
    mutateThreadMeta(threadId, { state: 'composing' });
    const current = getDraft(threadId);
    const restore: Partial<ComposeDraft> = {};
    if (current.text === '') restore.text = text;
    if (current.image_hashes.length === 0) restore.image_hashes = wireHashes;
    if (current.mode === null) restore.mode = mode;
    if (Object.keys(restore).length > 0) patchDraft(threadId, restore);
    throw err;
  }
}

/** Send a follow-up message in an already-active thread. Clears the local
 *  draft optimistically — the textarea is cleared synchronously by the input
 *  handler, but without a paired clear of the draft signal the textarea's
 *  useEffect resyncs the typed text on the next render and the "Discard
 *  draft" button stays visible. Mirrors the optimistic clear that sendCompose
 *  applies on the composing→active path. */
export async function sendFollowup(
  threadId: string,
  text: string,
  imageHashes?: string[],
  opts?: { useCodingAgent?: boolean; context?: ChatContext | null; focus?: boolean },
): Promise<void> {
  // Must run before the draft clear — see `markHashesAsSent`.
  if (imageHashes?.length) markHashesAsSent(imageHashes);
  updateCompose(threadId, { text: '', image_hashes: [] });
  lastSyncedImageHashes.delete(threadId);
  await sendMessage(text, imageHashes, { ...opts, threadId, focus: opts?.focus ?? true });
}

/** Tab-close-safe flush. Each pending PUT goes out with `keepalive: true` so
 *  the browser keeps the connection alive after the document tears down. The
 *  device-id header is critical: without it, the server's broadcast carries
 *  origin_device_id=None and other tabs/devices can't suppress the echo —
 *  potentially clobbering newer text that was typed elsewhere immediately
 *  after the close. */
function flushAllPending(): void {
  const headers: Record<string, string> = { 'Content-Type': 'application/json' };
  const deviceId = typeof localStorage !== 'undefined' ? localStorage.getItem('lucidos-device-id') : null;
  if (deviceId) headers['x-lucidos-device-id'] = deviceId;

  for (const [threadId, timer] of pendingTimers) {
    clearTimeout(timer);
    const thread = threadMap.value.get(threadId);
    if (!thread) continue;
    const draft = getDraft(threadId);
    // Always emit the full array on tab close — hashes are tiny.
    const body = JSON.stringify({
      text: draft.text,
      image_hashes: draft.image_hashes,
      mode: thread.meta.state === 'composing' ? draft.mode : null,
    });
    if (body.length > 64 * 1024) {
      // Telemetry carve-out (.claude/rules/frontend.md): the tab is unloading,
      // so any toast would never render. The next foreground page load will
      // re-PUT this draft via the normal debounce path — no data loss, the
      // draft is still in `composeDrafts` and gets retried as soon as the
      // user types again or focuses the thread.
      console.warn(`[compose] keepalive body exceeds 64KB (${body.length}B) for ${threadId}; will retry on next foreground push`);
      continue;
    }
    fetch(`/api/v1/threads/${encodeURIComponent(threadId)}/compose`, {
      method: 'PUT',
      headers,
      body,
      keepalive: true,
    }).catch(() => { /* tab is unloading */ });
  }
  pendingTimers.clear();
  // iOS bfcache can resume the page with frozen JS state; without this, stale
  // entries would block the next loadAllThreads from refreshing those threads.
  pendingComposePuts.clear();
}

if (typeof window !== 'undefined') {
  window.addEventListener('beforeunload', flushAllPending);
  window.addEventListener('pagehide', flushAllPending);
}
