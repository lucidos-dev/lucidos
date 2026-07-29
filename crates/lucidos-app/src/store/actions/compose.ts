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

import { threadMap, focusedThreadId, inputMode, showToast, showConfirm, setFocusedThread, selectedScope, type Scope } from '../store';
import { generateUuid } from '../../utils/uuid';
import {
  getComposeSelectionOverride,
  patchComposeSelection,
  clearComposeSelection,
  seedComposeSelection,
  takePendingComposeSelection,
  setComposeSelectionFromServer,
  resolveScope,
  resolveCodingAgent,
  resolveModel,
  resolveReasoningEffort,
  resolveCcModel,
  resolveCcReasoningEffort,
  type ComposeSelectionOverride,
} from '../composeSelections';
import type { ComposeDestination } from '../composeDestination';
import { makeOptimisticThreadState, type StoredEvent, type ThreadMeta } from '../thread-events';
import { clearDraft, composeDrafts, draftIsEmpty, getDraft, patchDraft, setDraft, type ComposeDraft } from '../composeDrafts';
import { API, ApiError, ensureThreadStarted, putComposeOnThread, deleteThread } from '../../api/client';
import { errorDetail } from '../../utils/errorDetail';
import { sendMessage } from './chat';
import type { ChatContext } from './chatContext';
import { markHashesAsSent } from '../../components/chat/pastedImages';
import { requestPromptOverrideSync } from '../../components/chat/promptValueSync';
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
  // Compose target: a focused composing draft (threadId), or the PENDING slot
  // for the not-yet-created draft (threadId null). An active thread has no
  // compose picker — ignore it defensively.
  const composing = !threadId || threadMap.value.get(threadId)?.meta.state === 'composing';
  if (!composing) return;
  // Per-draft (or pending) target. `patchComposeSelection(null, …)` routes to the
  // pending slot.
  if (d.kind === 'coding') {
    // Update the localStorage last-used scope seed (persisted by effects.ts) so
    // the NEXT new draft / the fresh compose view starts from this target. This
    // is leak-safe: `resolveScope` reads `selectedScope` ONLY for the no-draft
    // compose view — an existing draft resolves its OWN stored scope — so this
    // write can't move another draft (the bug the per-draft design fixed).
    if (!scopeEquals(selectedScope.value, d.scope)) {
      selectedScope.value = d.scope;
    }
    const current = getComposeSelectionOverride(threadId).scope;
    if (!current || !scopeEquals(current, d.scope)) {
      patchComposeSelection(threadId, { scope: d.scope });
      // A scope change with no accompanying mode change wouldn't otherwise fire a
      // compose PUT — persist the per-draft scope, and mark locally-edited so a
      // stale loadAllThreads snapshot can't revert it (mirrors updateComposeSelection).
      if (threadId) {
        markLocallyEdited(threadId);
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

/** Per-thread timestamp of the last compose PUT *settling* (Date.now()),
 *  stamped in pushNow's finally. Closes the inverse of the `composeEditedAt`
 *  hole: when the edit happened BEFORE a stale GET started (so
 *  `composeEditedAt` is older than the GET's request time) but the debounced
 *  PUT only settled AFTER the GET started, the GET's server snapshot was read
 *  before the PUT committed — yet by the time its response lands
 *  `pendingComposePuts` is already cleared. upsertThread consults this map and
 *  skips the overwrite when a PUT settled AT OR AFTER the GET went out. Without
 *  it, the "thread draft persists when switching to compose and back" flow
 *  intermittently blanks the restored draft (drafts.spec.ts:65). Stays set
 *  forever (no expiry); cross-device sync still works because a legitimate
 *  later refresh captures a request time AFTER the last local PUT settled. */
export const composePutSettledAt = new Map<string, number>();

/** Per-thread SERVER-time watermark captured at the last local compose edit:
 *  the newest `thread_summaries.last_activity` this device had seen for the
 *  thread at that moment (`meta.updatedAt`). Stamped together with
 *  `composeEditedAt` by `markLocallyEdited`, and what makes a *superseded
 *  draft* decidable WITHOUT ever comparing a client clock to a server clock —
 *  both sides of the test (`draftIsSuperseded`) are server timestamps. Absent =
 *  this device never authored the draft, so there is no reference point and
 *  nothing can supersede it (the existing clear paths own that case). */
export const composeEditWatermark = new Map<string, string>();

/** What the server holds for a thread's draft — the compose fields the
 *  supersede rule compares, minus the mode/selection it ignores. */
export interface ServerDraft {
  text: string;
  imageHashes: string[];
}

/** This device's knowledge of the draft the SERVER currently holds for the
 *  thread. Written ONLY where the server itself reports it: our own compose PUT
 *  succeeding (the server now holds exactly what we sent), a thread-summary
 *  snapshot, and a `ThreadComposeChanged` broadcast. Absent = never heard, so
 *  nothing is known.
 *
 *  It is the second half of the supersede rule, and the half that keeps a
 *  RE-TYPE safe when this device hadn't yet seen the submission: if the server
 *  still holds our draft, our PUT landed AFTER the submission cleared compose,
 *  so the draft is deliberate new work rather than the submission itself.
 *  Server-event ordering alone cannot tell those apart — the watermark is stale
 *  in exactly that case.
 *
 *  Deliberately NOT written from thread events, even though the projection
 *  clears the compose fields in the same transaction as `MessageReceived`: an
 *  event can be *delivered* long after it was written (a lagging stream, a
 *  throttled tab, replay on wake), so it is no evidence of what the server holds
 *  NOW — and letting a late one overwrite a newer PUT ack would re-open the very
 *  hole this map closes. Only a report the server made about its CURRENT compose
 *  state counts. */
export const serverDraft = new Map<string, ServerDraft>();

/** Record a server report of the thread's stored draft. Copies the hash array:
 *  callers hand over an array they also stage into the local draft, and the two
 *  records must not alias. */
export function noteServerDraft(threadId: string, text: string, imageHashes: readonly string[]): void {
  serverDraft.set(threadId, { text, imageHashes: [...imageHashes] });
}

function markLocallyEdited(threadId: string): void {
  composeEditedAt.set(threadId, Date.now());
  // `.peek()`, not `.value`: this runs from input/event handlers on the
  // keystroke path, and subscribing the caller to `threadMap` here would wake
  // every threadMap consumer per character (the whole reason draft text lives
  // outside threadMap in the first place).
  composeEditWatermark.set(threadId, threadMap.peek().get(threadId)?.meta.updatedAt ?? '');
}

/** The composer content a persisted event represents, or `null` when the event
 *  is not something the user's composer submitted. A deliberately CLOSED set:
 *  a sent message, and the two question answers that can carry typed text (the
 *  free-form answer path never emits `MessageReceived` — chat/process/run.rs
 *  reroutes typed text straight to `UserQuestionAnswered`). Agent- and
 *  engine-authored entries — `UserPromptInjected` above all — are NOT user
 *  submissions and must never supersede a draft. */
function submittedUserInput(event: StoredEvent): { text: string; imageHashes: string[] } | null {
  if (event.type === 'MessageReceived') {
    return { text: event.text ?? '', imageHashes: event.user_image_hashes ?? [] };
  }
  if (event.type === 'UserQuestionAnswered') {
    const { answer } = event;
    if (answer.kind === 'FreeText') return { text: answer.text, imageHashes: [] };
    // Multi-select folds the prompt textarea's text into the answer; an
    // options-only answer submitted no composer text at all.
    if (answer.kind === 'MultiSelected' && answer.text) return { text: answer.text, imageHashes: [] };
  }
  return null;
}

function sameHashes(a: readonly string[], b: readonly string[]): boolean {
  return a.length === b.length && a.every((h, i) => h === b[i]);
}

/** True when this device's draft is **superseded** — the server no longer holds
 *  it, AND its exact content (trimmed text + image hashes) went out as a
 *  submitted user input whose server `created` is strictly newer than the
 *  draft's edit watermark.
 *
 *  Content match ALONE is deliberately not enough: the user must stay free to
 *  post the same text many times. Two independent things make a re-type safe.
 *  Ordering covers the ordinary case — the watermark captured on the re-type
 *  already sits at/after the earlier submission's `created`, and the comparison
 *  is strict `>`. The server-state check covers the case ordering CANNOT see:
 *  if a peer submitted while this device was behind, the watermark is stale and
 *  the late event looks newer, but our own PUT then re-wrote the text
 *  server-side — so a server that still holds our draft is proof the draft
 *  post-dates the submission and is new work, not the thing that was sent.
 *
 *  Cheap by construction: the O(1) early-outs reject every thread without a
 *  locally-authored, server-cleared draft before the event scan, so nothing
 *  walks history on the keystroke path (never called from `updateCompose`). */
export function draftIsSuperseded(threadId: string): boolean {
  const draft = getDraft(threadId);
  if (draftIsEmpty(draft)) return false;
  const watermark = composeEditWatermark.get(threadId);
  if (watermark === undefined) return false;
  // A write of ours is in flight (or still inside the debounce), so what the
  // server holds is not yet knowable — every other compose guard yields to this
  // set for the same reason.
  if (pendingComposePuts.has(threadId)) return false;
  const text = draft.text.trim();
  const onServer = serverDraft.get(threadId);
  // Never heard from the server (e.g. the draft's PUT has only ever failed) —
  // the draft may be unsynced work, so it is not ours to drop.
  if (onServer === undefined) return false;
  // The server holds a live copy of THIS draft, so our PUT landed after the
  // submission cleared compose. Compared on text AND images, so an images-only
  // draft (both texts empty) isn't mistaken for one the server still has.
  const heldOnServer = onServer.text.trim() !== '' || onServer.imageHashes.length > 0;
  if (heldOnServer && onServer.text.trim() === text && sameHashes(draft.image_hashes, onServer.imageHashes)) {
    return false;
  }
  const thread = threadMap.peek().get(threadId);
  if (!thread) return false;
  for (const event of thread.events.values()) {
    // Missing `created` (a backend bug `handleEvent` already warns about)
    // cannot be ordered against the watermark — never treat it as evidence.
    if (!event.created || event.created <= watermark) continue;
    const submitted = submittedUserInput(event);
    if (!submitted) continue;
    if (submitted.text.trim() === text && sameHashes(draft.image_hashes, submitted.imageHashes)) return true;
  }
  return false;
}

/** Drop a draft the thread's own history proves was already submitted. Self-
 *  guarding, so any inbound path can fire it without pre-checking; it is the
 *  ACTIVE half of the supersede rule, for the paths that carry no compose-clear
 *  of their own — most importantly event replay on wake / SSE reconnect, where
 *  `loadAllThreads` runs BEFORE the missed messages arrive, so the
 *  empty-snapshot guard had no evidence to go on yet. */
export function clearSupersededDraft(threadId: string): void {
  if (!draftIsSuperseded(threadId)) return;
  clearDraft(threadId);
  // The projection wipes `compose_selection` alongside the text it cleared, so
  // the draft's dropdown picks died with it. Dropping them here keeps local
  // state from diverging — the replay path reaches this without ever passing
  // through `setComposeSelectionFromServer`, so nothing else would.
  clearComposeSelection(threadId);
}

/** True when this device holds UNSENT work for the thread: a non-empty draft it
 *  locally authored, whose content has NOT since been submitted. The shared
 *  invariant guard for every INBOUND compose EMPTY-clear path: the bulk
 *  `loadAllThreads` empty snapshot (`stageDraftFromApi`), an empty SSE
 *  `ThreadComposeChanged` (`applyRemoteCompose`), and the SSE `MessageReceived`
 *  echo's clear (thread-sync). Such a draft is the user's unsent intent and must
 *  never be blanked by an inbound echo/snapshot — only a send/discard FROM THIS
 *  DEVICE, or the proof that the draft has already been submitted, clears it.
 *  (`upsertThread` guards the distinct NON-empty stale overwrite with the
 *  `composeEditedAt` / `composePutSettledAt` / `pendingComposePuts` timestamps,
 *  not this helper.) `composeEditedAt` is stamped by `markLocallyEdited` and
 *  never cleared, so a server-ORIGINATED draft (present but never edited here)
 *  returns false and stays clearable by a genuine remote clear — and, without
 *  the supersede half, a locally-authored one would have stayed UNclearable
 *  forever, which is how a draft submitted from another device lived on here
 *  for hours (docs/plans/2026-07-28-superseded-compose-drafts.md). */
export function hasUnsentLocalDraft(threadId: string): boolean {
  return composeEditedAt.has(threadId)
    && !draftIsEmpty(getDraft(threadId))
    && !draftIsSuperseded(threadId);
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

/** Single entry point for a per-draft dropdown selection change (model,
 *  reasoning, coding agent, coding-agent model/effort). A real `threadId`
 *  patches the keyed override, marks the thread locally-edited (so a stale
 *  loadAllThreads snapshot can't revert the pick — same guard as text), and
 *  schedules the debounced compose PUT that carries the selection to the DB. A
 *  `null`/`undefined` id (fresh compose, no draft yet) writes only the pending
 *  slot; it's transferred + persisted when the draft is created. Scope has its
 *  own entry point (`applyDestination`) because it also updates the localStorage
 *  last-used seed. */
export function updateComposeSelection(
  threadId: string | null,
  patch: ComposeSelectionOverride,
): void {
  patchComposeSelection(threadId, patch);
  if (threadId) {
    markLocallyEdited(threadId);
    schedulePush(threadId);
  }
}

/** Apply an SSE ThreadComposeChanged from a peer device. Caller must check
 *  origin_device_id, pendingComposePuts, and focused-textarea guards before
 *  invoking. Replaces the draft wholesale — SSE carries the full snapshot.
 *  Skips unknown threads so a stray broadcast can't seed an orphan draft
 *  entry; empty payloads clear the entry instead of inflating the Map. */
export function applyRemoteCompose(
  threadId: string,
  fields: {
    text: string;
    image_hashes: string[];
    mode: ComposeMode | null;
    /** The draft's DB-backed per-draft selection (`ThreadComposeChanged.selection`),
     *  hydrated into `composeSelections` so a peer's dropdown change syncs in. */
    selection?: ComposeSelectionOverride | null;
  },
): void {
  if (!threadMap.value.has(threadId)) return;
  // The server just told us what it holds — record it whether or not we apply
  // the payload locally (the empty branch below may keep a local draft). The
  // caller already dropped our own echo and any in-flight local write, so this
  // is a peer's report. A frame delayed past a later PUT ack of ours could
  // still overwrite newer knowledge; that residual is accepted and self-heals
  // (the server keeps the text, so the next snapshot re-stages it) — see
  // `docs/code-review-priors.md` § Frontend for why the obvious guard is worse.
  noteServerDraft(threadId, fields.text, fields.image_hashes);
  if (fields.text === '' && fields.image_hashes.length === 0 && fields.mode === null) {
    // A remote EMPTY snapshot must never clear a non-empty draft this device
    // authored — the SSE mirror of stageDraftFromApi's guard (thread-loading.ts).
    // The only emitter is the compose PUT handler; a PUT that fired before the
    // device-id header was available broadcasts origin=None, which bypasses the
    // SSE self-echo suppression (thread-sync.ts only suppresses a PRESENT origin)
    // and lands here. Without this, that own/non-attributable empty echo blanks
    // the just-typed draft — the value='' face of drafts.spec.ts:65 (see
    // docs/plans/2026-06-28-drafts-sse-empty-clear-guard.md). Gate on
    // hasUnsentLocalDraft so a server-ORIGINATED draft (never edited here) is
    // still clearable by a genuine peer clear, and so is a locally-authored one
    // the thread's history shows was already submitted; the kept draft is
    // local-view only. The same guard covers the selection — a draft with
    // genuinely unsent work keeps its picks.
    if (hasUnsentLocalDraft(threadId)) return;
    clearDraft(threadId);
    setComposeSelectionFromServer(threadId, fields.selection);
    return;
  }
  setDraft(threadId, { text: fields.text, image_hashes: fields.image_hashes, mode: fields.mode });
  setComposeSelectionFromServer(threadId, fields.selection);
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

/** In-flight `pushNow` calls per thread. A PUT slower than the debounce is
 *  still running when the next one starts, so the first to finish must not
 *  release `pendingComposePuts` on behalf of the later one. */
const inFlightPushes = new Map<string, number>();

async function pushNow(threadId: string): Promise<void> {
  inFlightPushes.set(threadId, (inFlightPushes.get(threadId) ?? 0) + 1);
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
    // Persist the draft's per-draft dropdown selection alongside text/images/mode
    // so a reload rehydrates it. `undefined` (no local selection) omits the field
    // → backend COALESCE preserves. A present selection replaces the stored one;
    // re-sending the same object on a plain keystroke PUT is a harmless no-op.
    const selectionOverride = getComposeSelectionOverride(threadId);
    const selectionForPut: ComposeSelectionOverride | undefined =
      Object.keys(selectionOverride).length > 0 ? selectionOverride : undefined;
    await putComposeOnThread(
      threadId,
      draft.text,
      wireHashes,
      // Mode is only meaningful for composing threads — once active, the
      // channel field is authoritative and the server rejects mode changes.
      thread.meta.state === 'composing' ? draft.mode : null,
      selectionForPut,
    );
    // Acked: the server now holds exactly what we sent. Recording it here is
    // what keeps a draft typed while this device was behind a peer's submission
    // from being mistaken for that submission — see `serverDraft`. A `null`
    // wireHashes preserved the stored array, which is `draft.image_hashes` by
    // construction (that's the condition for sending `null`).
    noteServerDraft(threadId, draft.text, draft.image_hashes);
    if (wireHashes !== null) {
      lastSyncedImageHashes.set(threadId, wireHashes);
    }
  } finally {
    const stillRunning = (inFlightPushes.get(threadId) ?? 1) - 1;
    if (stillRunning > 0) inFlightPushes.set(threadId, stillRunning);
    else inFlightPushes.delete(threadId);
    // Release ONLY when nothing newer is queued or still running. An edit made
    // during this PUT scheduled a fresh debounce (and a slow PUT can overlap
    // the next one outright); dropping the flag here would advertise "the
    // server has seen our latest intent" while a later write is still pending —
    // which is exactly what every consumer of this set reads it to mean.
    if (stillRunning === 0 && !pendingTimers.has(threadId)) pendingComposePuts.delete(threadId);
    // Stamp the settle moment AFTER clearing pendingComposePuts: from here a
    // GET that started before this settle (its server snapshot read before our
    // PUT committed) must not clobber local compose state. See
    // `composePutSettledAt`. Set even on the awaitThreadStarted early-return /
    // PUT-failure path — local is still the user's latest intent, and a failed
    // sync already toasted.
    composePutSettledAt.set(threadId, Date.now());
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
  id = generateUuid();
  setFocusedThread(id);
  // Seed THIS new draft's own stored selection: eager-copy the localStorage
  // last-used scope so the draft carries a scope in its OWN override (resolveScope
  // no longer falls back to the shared `selectedScope` for a real draft — that's
  // the leak guard, so the new draft must own its scope), overlaid with any
  // fresh-compose picks from the pending slot (a pending scope pick wins). Other
  // fields stay unset and resolve to their account defaults. The seeded selection
  // is persisted by the first keystroke's compose PUT (pushNow includes it).
  seedComposeSelection(id, { scope: selectedScope.value, ...takePendingComposeSelection() });
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

/** Prefill the compose input with a starter prompt (the welcome suggestions).
 *  Lazily focuses/creates a composing thread, then writes the text through the
 *  normal debounced compose path so it syncs to the textarea and persists like
 *  any typed draft. Replaces the WHOLE input — text AND any attached images —
 *  so the starter lands cleanly; a lingering attachment would otherwise ride
 *  along with the unrelated suggestion (`image_hashes: []` is a no-op on a
 *  brand-new draft). Does NOT send — the user reviews/edits, picks a
 *  destination, and hits Send themselves. Returns the thread id the text
 *  landed on. */
export function prefillCompose(text: string): string {
  const threadId = ensureFocusedComposeThread();
  updateCompose(threadId, { text, image_hashes: [] });
  return threadId;
}

/** Apply a welcome-message starter suggestion to the compose input.
 *
 *  Starter suggestions are conversational, so the destination is forced to the
 *  Lucidos Agent (a coding-agent draft flips back to chat). The suggestion
 *  REPLACES the focused draft's whole input — text AND any attached images (via
 *  `prefillCompose`) — so if a non-empty draft is already in progress, confirm the
 *  override first (a click must never silently blow away typed text or attachments;
 *  declining keeps the draft untouched). The override is force-synced
 *  into the textarea via `requestPromptOverrideSync` because the normal
 *  compose→textarea sync skips a focused, non-empty input to protect in-flight
 *  typing — without the force the draft signal (and the drawer row) would update
 *  but the visible prompt would stay stale.
 *
 *  Returns true when the suggestion was applied, false when the user declined the
 *  override. Does NOT send — the user reviews/edits and hits Send themselves. */
export async function applySuggestion(text: string): Promise<boolean> {
  const existingId = focusedThreadId.value;
  if (existingId && !draftIsEmpty(getDraft(existingId))) {
    const ok = await showConfirm(
      'You have a draft in progress. Replace it with this suggestion?',
      'Replace',
      { title: 'Replace draft?', cancelLabel: 'Keep my draft' },
    );
    if (!ok) return false;
  }
  // Target the Lucidos Agent. Set BEFORE prefill so a brand-new draft is born on
  // the chat channel, and so an existing coding-agent draft flips back to chat.
  applyDestination(focusedThreadId.value, { kind: 'lucidos-agent' });
  prefillCompose(text);
  requestPromptOverrideSync();
  return true;
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
  // Drop the per-draft dropdown overrides too — a discarded draft is gone, and a
  // stray entry would seed a future draft that happens to reuse the id.
  clearComposeSelection(threadId);
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
  // (see frontend.md "Drafts Are Threads"). Every dropdown value is resolved
  // from THIS draft's per-draft override (composeSelections), falling back to
  // the current global default — never read straight off a global signal that
  // another draft may have changed. Channel is locked from `opts.useCodingAgent`
  // (which the caller resolved via effectiveSendMode) rather than the existing
  // meta.channel: the latter was stamped at first-keystroke time from
  // `currentComposeMode()` and goes stale the moment the user toggles. Without
  // this lock, sendMessage reads the stale channel, ignores the explicit
  // useCodingAgent option, and routes a coding-agent send through the Lucidos
  // Agent (or vice versa).
  const scope = resolveScope(threadId);
  const boundRepoId = opts.useCodingAgent && scope.kind === 'external' ? scope.repoId : undefined;
  const boundCodingAgentKind = opts.useCodingAgent ? scope.kind : undefined;
  const boundCodingAgentFolder = opts.useCodingAgent && scope.kind === 'app'
    ? `data/apps/${scope.appId}`
    : undefined;
  const boundChannel: ThreadMeta['channel'] = opts.useCodingAgent ? 'claude_code' : 'chat';
  const boundCodingAgent = opts.useCodingAgent ? resolveCodingAgent(threadId) : undefined;
  mutateThreadMeta(threadId, {
    state: 'active',
    channel: boundChannel,
    repoId: boundRepoId,
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
  const reasoningEffortOverride = resolveReasoningEffort(threadId);
  const ccModelOverride = resolveCcModel(threadId);
  const ccReasoningEffortOverride = resolveCcReasoningEffort(threadId);
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
      modelOverride,
      reasoningEffortOverride,
      ccModelOverride,
      ccReasoningEffortOverride,
    });
    // A compose-view pick is a one-shot intent: it has now been carried into this
    // spawn's chat body, so consume the draft's per-draft selection. Without this
    // the override would linger in `composeSelections` for a thread that is no
    // longer composing. Follow-ups (sendFollowup) don't go through here — an
    // active thread has no compose selection entry.
    clearComposeSelection(threadId);
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
    // Include the per-draft selection so a dropdown pick made within the debounce
    // window right before tab close isn't lost — parity with the text/images flush.
    // Omit when empty so the backend COALESCE preserves the stored value.
    const selectionOverride = getComposeSelectionOverride(threadId);
    const selectionForFlush = Object.keys(selectionOverride).length > 0 ? selectionOverride : undefined;
    // Always emit the full array on tab close — hashes are tiny.
    const body = JSON.stringify({
      text: draft.text,
      image_hashes: draft.image_hashes,
      mode: thread.meta.state === 'composing' ? draft.mode : null,
      selection: selectionForFlush,
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
    // `API` carries the gateway base prefix (`/<slug>/api/v1`); a bare
    // `/api/v1/...` would make the gateway read `api` as a workspace slug and
    // 404 ("unknown workspace 'api'"), silently dropping the tab-close flush
    // for every gateway-served workspace.
    fetch(`${API}/threads/${encodeURIComponent(threadId)}/compose`, {
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
