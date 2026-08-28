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
 * The SSE consumer in thread-sync.ts applies remote ThreadStarted /
 * ThreadDiscarded / ThreadComposeChanged with origin_device_id and
 * focused-textarea guards.
 */

import { threadMap, focusedThreadId, inputMode, showToast, removeToast, showConfirm, setFocusedThread, selectedScope, repositories, type Scope } from '../store';
import { loadedOr } from '../types';
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
import { scopeRepoName, type ComposeDestination } from '../composeDestination';
import { makeOptimisticThreadState, type StoredEvent, type ThreadMeta } from '../thread-events';
import { clearDraft, composeDrafts, draftIsEmpty, getDraft, patchDraft, setDraft, type ComposeDraft } from '../composeDrafts';
import { API, ApiError, ensureThreadStarted, putComposeOnThread, deleteThread, isTransientFetchError, type ComposePutResult } from '../../api/client';
import { errorDetail } from '../../utils/errorDetail';
import { createFailureCounter } from '../../utils/failureCounter';
import { instantMicros } from '../../utils/isoInstant';
import { sendMessage } from './chat';
// Cycle-safe: `compose -> chat -> thread-loading -> compose` already exists, and
// this is a function declaration called at runtime, never at module init.
import { forgetThreadEventsFailures } from './thread-loading';
// Same shape of cycle (`compose -> threads -> thread-loading -> compose`), same
// reason it is safe: called at runtime, never at module init.
import { unfocusThread } from './threads';
import type { ChatContext } from './chatContext';
// Not a new edge at all: `./chat` above already imports this same function, so
// `compose -> pane` was in the graph before this line. The property to re-check
// is that nothing `pane` reaches comes back to `compose`. Its direct import list
// is not that check: it pulls in 25 action modules through the drawer.
import { revealThreadPane } from './pane';
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
      // Persist the per-draft scope, and mark locally-edited so a stale
      // loadAllThreads snapshot cannot revert it.
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
 *  peer device sees the change within an eye-blink. Long enough that a
 *  continuous typist does not flood the engine and SSE fan-out per character. */
const DEBOUNCE_MS = 250;

const pendingTimers = new Map<string, ReturnType<typeof setTimeout>>();

/** Thread ids with a pending PUT. Covers the whole window from the optimistic
 *  local write to the server PUT's ack, the debounce included. SSE and
 *  loadAllThreads consult this set to avoid clobbering a local change the
 *  server has not seen yet.
 *
 *  Covering the DEBOUNCE matters on an iOS PWA. PHPicker dismissal fires
 *  visibilitychange, and so loadAllThreads, at roughly the same instant as the
 *  file input's change event. Without it, a freshly attached image is
 *  overwritten by the server's stale empty array before the PUT goes out. */
export const pendingComposePuts = new Set<string>();

/** Per-thread timestamp of the last local compose mutation (Date.now()).
 *  loadAllThreads and ensureThread* capture their own request start time and
 *  consult this map. A thread edited AFTER the GET started makes the response
 *  stale for compose state, so the overwrite is skipped.
 *
 *  Without it, a GET issued before the user's photo attach, yet landing after
 *  pushNow's PUT clears `pendingComposePuts`, overwrites the optimistic image.
 *  Stays set forever, with no expiry. Cross-device sync still works, a
 *  legitimate refresh capturing a request time after the last local edit. */
export const composeEditedAt = new Map<string, number>();

/** Per-thread timestamp of the last compose PUT settling (Date.now()), stamped
 *  in pushNow's finally. Closes the inverse of the `composeEditedAt` hole: an
 *  edit made BEFORE a stale GET started, whose debounced PUT settled only
 *  AFTER. The GET's server snapshot was read before the PUT committed, yet
 *  `pendingComposePuts` is already cleared by the time its response lands.
 *
 *  upsertThread consults this map and skips the overwrite when a PUT settled at
 *  or after the GET went out. Stays set forever, with no expiry. Cross-device
 *  sync still works, a legitimate later refresh capturing a request time after
 *  the last local PUT settled. */
export const composePutSettledAt = new Map<string, number>();

/** Per-thread SERVER-time watermark captured at the last local compose edit:
 *  the newest `thread_summaries.last_activity` this device had seen for the
 *  thread (`meta.updatedAt`). Stamped with `composeEditedAt` by
 *  `markLocallyEdited`.
 *
 *  It makes a *superseded draft* decidable without ever comparing a client
 *  clock to a server clock, both sides of `draftIsSuperseded` being server
 *  timestamps. Absent means this device never authored the draft, so there is
 *  no reference point and nothing can supersede it. */
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
 *  It is the second half of the supersede rule. It is also the half that keeps
 *  a RE-TYPE safe when this device had not yet seen the submission. A server
 *  still holding our draft proves our PUT landed AFTER the submission cleared
 *  compose. The draft is then new work, not the submission itself. Server-event
 *  ordering alone cannot tell those apart, the watermark being stale in exactly
 *  that case.
 *
 *  **Deliberately NOT written from thread events**, even though the projection
 *  clears the compose fields in the same transaction as `MessageReceived`. An
 *  event can be delivered long after it was written, so it is no evidence of
 *  what the server holds NOW. Letting a late one overwrite a newer PUT ack
 *  would re-open the hole this map closes. Only a report the server made about
 *  its CURRENT compose state counts. */
export const serverDraft = new Map<string, ServerDraft>();

/** Record a server report of the thread's stored draft. Copies the hash array:
 *  callers hand over an array they also stage into the local draft, and the two
 *  records must not alias. */
export function noteServerDraft(threadId: string, text: string, imageHashes: readonly string[]): void {
  serverDraft.set(threadId, { text, imageHashes: [...imageHashes] });
}

/** This device's knowledge of each thread's *compose epoch*
 *  (`docs/glossary.md`): how many times a submission has consumed the thread's
 *  compose slot. Echoed on every compose PUT, so the engine can refuse a write
 *  composed before a submission that has since landed. That stops a stalled
 *  draft PUT from resurrecting text a send already consumed.
 *
 *  Absent means never heard, so the PUT goes out unfenced. That is the honest
 *  reading of "we do not know", and it matches how the engine treats a missing
 *  epoch. Learned from three places, all of them the engine reporting its own
 *  state: a thread-summary snapshot, a `ThreadComposeChanged` broadcast, and
 *  the `412` that refuses a stale write. */
const composeEpoch = new Map<string, number>();

/** Record an engine report of the thread's compose epoch. Monotonic: a frame
 *  delayed past a newer one must not walk the value backwards. That would make
 *  the next write fail a fence it had already cleared. */
export function noteComposeEpoch(threadId: string, epoch: number | undefined): void {
  if (typeof epoch !== 'number') return;
  const known = composeEpoch.get(threadId);
  if (known !== undefined && epoch <= known) return;
  composeEpoch.set(threadId, epoch);
}

/** Test-only: what this device believes the engine's compose epoch to be. */
export function _composeEpochForTesting(threadId: string): number | undefined {
  return composeEpoch.get(threadId);
}

function markLocallyEdited(threadId: string): void {
  composeEditedAt.set(threadId, Date.now());
  // `.peek()`, not `.value`. This runs from input handlers on the keystroke
  // path, and subscribing the caller to `threadMap` here would wake every
  // threadMap consumer per character. That is why draft text lives outside
  // threadMap at all.
  composeEditWatermark.set(threadId, threadMap.peek().get(threadId)?.meta.updatedAt ?? '');
}

/** The composer content a persisted event represents, or `null` when the event
 *  is not something the user's composer submitted. A deliberately CLOSED set:
 *  a sent message, and the two question answers that can carry typed text. The
 *  free-form answer path emits no `MessageReceived`, chat/process/run.rs
 *  rerouting typed text straight to `UserQuestionAnswered`.
 *
 *  Agent- and engine-authored entries, `UserPromptInjected` above all, are NOT
 *  user submissions and must never supersede a draft. */
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

/** True when this device's draft is **superseded**. Two conditions. The server
 *  no longer holds it. And its exact content went out as a submitted user
 *  input whose server `created` is strictly newer than the edit watermark.
 *
 *  **Content match ALONE is deliberately not enough**: the user must stay free
 *  to post the same text many times. Two independent things make a re-type
 *  safe. Ordering covers the ordinary case, the watermark captured on the
 *  re-type already sitting at or after the earlier submission's `created`.
 *
 *  The server-state check covers the case ordering CANNOT see. A peer
 *  submitting while this device was behind leaves the watermark stale, so the
 *  late event looks newer. Our own PUT then re-wrote the text server-side, so a
 *  server still holding our draft proves the draft post-dates the submission.
 *
 *  Cheap by construction. The O(1) early-outs reject every thread without a
 *  locally-authored, server-cleared draft before the event scan, so nothing
 *  walks history on the keystroke path. */
export function draftIsSuperseded(
  threadId: string,
  opts?: { writeRefused?: boolean },
): boolean {
  const draft = getDraft(threadId);
  if (draftIsEmpty(draft)) return false;
  const watermark = composeEditWatermark.get(threadId);
  if (watermark === undefined) return false;
  // A write of ours is in flight, or still inside the debounce, so what the
  // server holds is not yet knowable. Every other compose guard yields to this
  // set for the same reason.
  //
  // `writeRefused` is the one case where that uncertainty is already resolved.
  // The engine answered our in-flight write with a stale-epoch `412`. That
  // says both that it did NOT apply the write and that a submission consumed
  // the slot. Without the opt-out this rule is DEAD on that path, a write in
  // flight for the whole life of `pushNow` by construction. A draft the user
  // already sent from another device would then be re-pushed by the retry.
  if (!opts?.writeRefused && pendingComposePuts.has(threadId)) return false;
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
  // A thread that never reported an activity time carries an empty
  // `updatedAt`, and the floor keeps what the string compare here used to do:
  // every real timestamp is newer than it.
  const watermarkMicros = instantMicros(watermark) ?? -Infinity;
  for (const event of thread.events.values()) {
    // Missing `created` (a backend bug `handleEvent` already warns about)
    // cannot be ordered against the watermark — never treat it as evidence.
    const createdMicros = instantMicros(event.created);
    if (createdMicros === null || createdMicros <= watermarkMicros) continue;
    const submitted = submittedUserInput(event);
    if (!submitted) continue;
    if (submitted.text.trim() === text && sameHashes(draft.image_hashes, submitted.imageHashes)) return true;
  }
  return false;
}

/** Drop a draft the thread's own history proves was already submitted.
 *  Self-guarding, so any inbound path can fire it without pre-checking.
 *
 *  It is the ACTIVE half of the supersede rule, for the paths carrying no
 *  compose-clear of their own. Event replay on wake or SSE reconnect matters
 *  most: `loadAllThreads` runs BEFORE the missed messages arrive, so the
 *  empty-snapshot guard has no evidence to go on yet. */
export function clearSupersededDraft(
  threadId: string,
  opts?: { writeRefused?: boolean },
): boolean {
  if (!draftIsSuperseded(threadId, opts)) return false;
  clearDraft(threadId);
  // The thread's own history proves this draft was already submitted, so a
  // later flush must not push it back onto the server. Nothing was delivered,
  // so this is a drop rather than a settle.
  dropUndeliveredComposeDraft(threadId);
  // The projection wipes `compose_selection` alongside the text it cleared, so
  // the draft's dropdown picks died with it. Dropping them here keeps local
  // state from diverging — the replay path reaches this without ever passing
  // through `setComposeSelectionFromServer`, so nothing else would.
  clearComposeSelection(threadId);
  return true;
}

/** True when this device holds UNSENT work for the thread: a non-empty draft it
 *  locally authored, whose content has NOT since been submitted.
 *
 *  The shared guard for every INBOUND compose EMPTY-clear path. Those are the
 *  bulk `loadAllThreads` empty snapshot (`stageDraftFromApi`), an empty SSE
 *  `ThreadComposeChanged` (`applyRemoteCompose`), and the SSE `MessageReceived`
 *  echo's clear (thread-sync). **Such a draft is the user's unsent intent and
 *  must never be blanked by an inbound echo or snapshot.** Only a send or
 *  discard FROM THIS DEVICE clears it, or proof that it was already submitted.
 *  `upsertThread` guards the distinct NON-empty stale overwrite with the
 *  timestamp maps above, not with this helper.
 *
 *  `composeEditedAt` is stamped by `markLocallyEdited` and never cleared. So a
 *  server-ORIGINATED draft returns false and stays clearable by a genuine
 *  remote clear. Without the supersede half, a locally-authored one would stay
 *  UNclearable forever
 *  (docs/plans/2026-07-28-superseded-compose-drafts.md). */
export function hasUnsentLocalDraft(threadId: string): boolean {
  return composeEditedAt.has(threadId)
    && !draftIsEmpty(getDraft(threadId))
    && !draftIsSuperseded(threadId);
}

/** Single entry point for compose mutations from the UI. Optimistic local
 *  apply, then debounced server PUT. A composing thread left empty by the patch
 *  is auto-discarded: never sent and with no content, it would otherwise render
 *  as a ghost row titled "Empty draft". updatedAt is deliberately left alone,
 *  typing a draft being no activity the drawer should surface. The backend's
 *  last-activity allowlist agrees. */
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
 *  reasoning, coding agent, coding-agent model and effort).
 *
 *  A real `threadId` patches the keyed override and schedules the debounced
 *  compose PUT. It also marks the thread locally-edited, so a stale
 *  loadAllThreads snapshot cannot revert the pick. A `null` id (fresh compose,
 *  no draft yet) writes only the pending slot, transferred and persisted when
 *  the draft is created. Scope has its own entry point (`applyDestination`),
 *  because it also updates the localStorage last-used seed. */
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
  // The server just told us what it holds, so record it whether or not the
  // payload is applied locally. The caller already dropped our own echo and any
  // in-flight local write, so this is a peer's report. A frame delayed past a
  // later PUT ack of ours could still overwrite newer knowledge. That residual
  // is accepted and self-heals, the server keeping the text. See
  // `docs/code-review-priors.md` for why the obvious guard is worse.
  noteServerDraft(threadId, fields.text, fields.image_hashes);
  // The broadcast states the stored mode too, on the same footing and recorded
  // before the same guards.
  noteServerComposeMode(threadId, fields.mode);
  if (fields.text === '' && fields.image_hashes.length === 0 && fields.mode === null) {
    // **A remote EMPTY snapshot must never clear a non-empty draft this device
    // authored.** The SSE mirror of stageDraftFromApi's guard
    // (thread-loading.ts). The only emitter is the compose PUT handler. A PUT
    // fired before the device-id header was available broadcasts origin=None,
    // which bypasses the SSE self-echo suppression and lands here (see
    // docs/plans/2026-06-28-drafts-sse-empty-clear-guard.md).
    //
    // Gate on hasUnsentLocalDraft. A server-ORIGINATED draft then stays
    // clearable by a genuine peer clear, as does a locally-authored one the
    // thread's history shows was already submitted. The kept draft is
    // local-view only. The same guard covers the selection: a draft with
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
 *  Distinct from `pendingComposePuts` above, which is the in-flight/debouncing
 *  marker every inbound clobber guard yields to. That one means "a write is on
 *  its way"; this one means "a write was owed and did not land".
 *
 *  Holds only the thread id, not a snapshot: the flush re-reads the CURRENT
 *  draft through `getDraft`, so last-write-wins is structural on the SEND side
 *  and no stale value can go out. Deciding what an ANSWER is allowed to conclude
 *  still needs a sequence, because overlapping PUTs can complete out of order:
 *  see `latestComposePushSeq`. */
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
  // count resets and the thread stops being owed a re-send.
  settleComposeDelivered(threadId);
  showToast(`Compose sync failed: ${errorDetail(err)}`, 'error', {
    key: COMPOSE_REJECTED_TOAST,
  });
}

/** Stop owing this thread a re-send, WITHOUT claiming the engine answered.
 *  For the paths where the draft itself stopped existing (sent, discarded,
 *  proven superseded), where a flush would push content the user is done with.
 *  Deliberately does not touch the failure counter: nothing was delivered. */
function dropUndeliveredComposeDraft(threadId: string): void {
  if (!undeliveredComposeDrafts.delete(threadId)) return;
  if (undeliveredComposeDrafts.size === 0) removeToast(COMPOSE_UNREACHABLE_TOAST);
}

/** Re-send every draft the engine never accepted. Called from `useStartup`'s
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
  runningComposePushes.clear();
  owedComposePushes.clear();
  composeEpoch.clear();
  lastSyncedComposeMode.clear();
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
  // Mark pending before the debounce so concurrent loads don't clobber.
  pendingComposePuts.add(threadId);
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
      // Release ONLY when nothing newer is queued. An edit made during the last
      // PUT scheduled a fresh debounce. Dropping the flag would then claim the
      // engine has seen our latest intent while a later write is pending. That
      // claim is what every consumer of this set reads it to mean.
      if (!pendingTimers.has(threadId)) pendingComposePuts.delete(threadId);
    }
  })();
}

/** Last image-hash array synced per thread. An unchanged array sends `null`, so
 *  the server's COALESCE preserves it and skips the SSE re-broadcast. */
const lastSyncedImageHashes = new Map<string, string[]>();

function imageHashesUnchanged(threadId: string, current: string[]): boolean {
  const prev = lastSyncedImageHashes.get(threadId);
  if (!prev) return false;
  if (prev.length !== current.length) return false;
  return prev.every((h, i) => h === current[i]);
}

/** What the ENGINE is known to hold as each thread's stored compose mode. The
 *  mode sibling of `lastSyncedImageHashes` above: an unchanged value goes out as
 *  `null` and the server's COALESCE preserves it.
 *
 *  Written only where the engine reports its own value, the same rule
 *  `serverDraft` follows. Four sites: the `POST /threads` ack, our own PUT ack,
 *  a thread-summary snapshot, and a `ThreadComposeChanged` broadcast. The POST
 *  counts because that handler 409s on a different mode, so an OK proves the
 *  stored value is ours. Absent means never heard, so the write carries the
 *  mode. Not knowing therefore costs one redundant field and can never drop a
 *  channel pick. */
const lastSyncedComposeMode = new Map<string, ComposeMode | null>();

/** Record an engine report of the thread's stored compose mode. */
export function noteServerComposeMode(threadId: string, mode: ComposeMode | null): void {
  lastSyncedComposeMode.set(threadId, mode);
}

/** The `mode` a compose write should carry. Only a composing draft has one to
 *  state: once sent, the channel is locked server-side and the picker is hidden.
 *  A pick the engine already holds goes out as `null`.
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
  if (lastSyncedComposeMode.get(threadId) === draft.mode) return null;
  return draft.mode;
}

/** One compose write, reading the draft at the moment it goes out. Only ever
 *  called from `runComposePushes`, which guarantees no sibling write is in
 *  flight for the same thread. `staleRetries` counts the re-issues this runner
 *  cycle has already spent adopting a newer epoch. */
async function pushNow(threadId: string, staleRetries = 0): Promise<void> {
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
    // SSE confirms the DELETE, so existence alone is not enough: a PUT here
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
        composeEpoch.get(threadId),
      );
    } catch (err) {
      handleComposePushFailure(threadId, err);
      return;
    }
    if (result.status === 'stale') {
      // A submission consumed the compose slot after this write was composed,
      // so the engine dropped it. Nothing is wrong and nothing is owed to the
      // user: adopt the epoch and re-issue.
      noteComposeEpoch(threadId, result.composeEpoch);
      // The refusal is also a report about the engine's compose state, and the
      // only one that arrives while a write of ours is in flight. The
      // submission that moved the epoch cleared the stored draft in the same
      // transaction, and this write was not applied, so the engine holds
      // nothing. Recording it is what lets the supersede rule below see past
      // its own "the server still has our text" re-type protection.
      noteServerDraft(threadId, '', []);
      // Give that rule its say before re-issuing. The submission may have BEEN
      // this draft, sent from another device. The retry would then put the sent
      // message back as a live draft on every device.
      if (clearSupersededDraft(threadId, { writeRefused: true })) return;
      if (staleRetries < MAX_STALE_EPOCH_RETRIES) {
        await pushNow(threadId, staleRetries + 1);
        return;
      }
      // Something keeps consuming the slot faster than we can write. Park it
      // rather than spin; the next resume flush re-reads the current draft.
      undeliveredComposeDrafts.add(threadId);
      return;
    }
    // Acked: the server now holds exactly what we sent. Recording it here is
    // what keeps a draft typed while this device was behind a peer's submission
    // from being mistaken for that submission (see `serverDraft`). A `null`
    // wireHashes preserved the stored array, which is `draft.image_hashes` by
    // construction (that's the condition for sending `null`).
    noteServerDraft(threadId, draft.text, draft.image_hashes);
    if (wireHashes !== null) {
      lastSyncedImageHashes.set(threadId, wireHashes);
    }
    // A `null` mode asked the server to preserve, so it reported nothing and we
    // learned nothing. Only a mode we actually sent is now known to be stored.
    if (wireMode !== null) {
      noteServerComposeMode(threadId, wireMode);
    }
    settleComposeDelivered(threadId);
  } finally {
    // From here a GET that started before this settle (its server snapshot read
    // before our PUT committed) must not clobber local compose state. See
    // `composePutSettledAt`. Set even on the awaitThreadStarted early-return and
    // the PUT-failure path: local is still the user's latest intent, and a
    // failed sync already toasted.
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
  try {
    await ensureThreadStarted(threadId, mode);
  } catch (err) {
    rollbackOptimistic(threadId);
    throw err;
  }
  // The row now holds this mode: `ThreadStarted` stores it, and the endpoint
  // 409s rather than accepting a different one. So the first keystroke write
  // has nothing to state about the channel.
  noteServerComposeMode(threadId, mode);
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
}

/** In-flight POST /threads promises keyed by thread id. Callers needing the row
 *  to exist server-side before issuing their own request consult this via
 *  `awaitThreadStarted`. Image blob upload is the one attach path that fires
 *  synchronously, with no debounce to hide the race. */
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
  // Seed THIS new draft's own stored selection. Eager-copy the localStorage
  // last-used scope, so the draft carries a scope in its OWN override, then
  // overlay any fresh-compose picks from the pending slot. A pending scope pick
  // wins. The eager copy is required: `resolveScope` no longer falls back to
  // the shared `selectedScope` for a real draft, which is the leak guard. Other
  // fields stay unset and resolve to their account defaults. The seeded
  // selection is persisted by the first keystroke's compose PUT.
  seedComposeSelection(id, { scope: selectedScope.value, ...takePendingComposeSelection() });
  // Inlined instead of focusThread(): that also fires loadThreadEvents and
  // (on mobile) navigateToPane, neither of which the draft path wants.
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
 *  first. `applySuggestion` does, and has to do it there rather than here,
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
 *  A focused DRAFT is left alone. Replacing it in place, after the confirm, is
 *  what a suggestion is supposed to do. `unfocusThread` rather than a bare
 *  `setFocusedThread(null)`, so the leaving thread's coding-agent pending picks
 *  are reset and the thread pane is revealed. That is how a mobile user tapping
 *  the header button reaches the conversation that starts. */
function dropNonComposingFocus(): void {
  const id = focusedThreadId.value;
  if (!id) return;
  if (threadMap.value.get(id)?.meta.state === 'composing') return;
  unfocusThread();
}

/** Apply a suggested sentence to the compose input on the user's behalf.
 *
 *  Its one caller is {@link sendSeededPrompt}, which reuses every part of it and
 *  only adds the send. It stays a separate step because the seeding and the send
 *  fail independently: a declined confirm must stop before either the pane moves
 *  or a message goes out.
 *
 *  A suggested sentence is conversational, so the destination is forced to the
 *  Lucidos Agent, a coding-agent draft flipping back to chat. It REPLACES the
 *  focused draft's whole input, text and attached images, via
 *  `prefillCompose`. A non-empty draft already in progress therefore needs the
 *  override confirmed first, since a click must never blow away typed text.
 *  Declining keeps the draft untouched.
 *
 *  The override is force-synced into the textarea via
 *  `requestPromptOverrideSync('replace')`. The normal sync skips a focused,
 *  non-empty input, to protect in-flight typing. Without the force, the draft
 *  signal would update while the visible prompt stayed stale. `'replace'` also
 *  end-snaps the caret: the old offset indexes text that is gone, so restoring
 *  it would drop the user inside the seeded sentence.
 *
 *  Returns true when the sentence was applied, false when the user declined the
 *  override. Does NOT send: that is {@link sendSeededPrompt}'s extra step. */
export async function applySuggestion(text: string): Promise<boolean> {
  dropNonComposingFocus();
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
  requestPromptOverrideSync('replace');
  return true;
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

/** Start the setup interview: seed {@link SETUP_INTERVIEW_PROMPT} and SEND it.
 *
 *  Unlike {@link applySuggestion}, this does not stop at the draft. A first-run
 *  user staring at a prefilled box they did not write has to decide whether to
 *  send it. That hesitation is what the entry point exists to remove, so the
 *  click is the whole gesture. Nothing is hidden by sending: the seeded
 *  sentence is what appears in the transcript, on the same code path a typed
 *  message takes.
 *
 *  Returns true when the interview was sent, false when the user declined the
 *  draft override or no draft resolved. */
export async function startSetupInterview(): Promise<boolean> {
  return sendSeededPrompt(SETUP_INTERVIEW_PROMPT, 'start the setup interview');
}

/** Seed `text` into the composer and SEND it, as the user.
 *
 *  The gesture behind every button that starts a conversation on the user's
 *  behalf: the first-run welcome's setup interview, a *release notice* action,
 *  and Discuss on a notification. Each hands the reader a sentence they can see,
 *  reword and re-send by typing, which is the prompt-first side of
 *  `docs/philosophy.md` principle 3.
 *
 *  It reveals the thread pane, because a send that starts a conversation lands
 *  on a thread (`.claude/rules/frontend.md` § Navigation That Lands Content).
 *  `applySuggestion` covers only the branch where it unfocuses, and `sendCompose`
 *  passes an explicit thread id, so `sendMessage`'s raw-new reveal never fires.
 *  Without this, a caller in the content pane sent into a thread the user could
 *  not see.
 *
 *  `what` completes "Failed to …" in the error toast, so write it as a verb
 *  phrase. Returns false when the user declined the draft override, when no
 *  draft resolved, or when the send failed. */
export async function sendSeededPrompt(text: string, what: string): Promise<boolean> {
  if (!(await applySuggestion(text))) return false;
  const threadId = focusedThreadId.value;
  if (!threadId) return false;
  // Before the send, not after. `ensureFocusedComposeThread` allocated the id
  // client-side, so the thread can be on screen while the request is still in
  // flight, rather than after a round trip.
  revealThreadPane();
  try {
    await sendCompose(threadId, { focus: true });
  } catch (err) {
    // `sendCompose` rethrows after rolling the draft back, and its other callers
    // toast (see `beginSend` in PromptInput). Every entry point here fires this
    // as `void`, so swallowing would surface a failed click as nothing
    // happening at all.
    showToast(`Failed to ${what}: ${errorDetail(err)}`, 'error');
    return false;
  }
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
  // Drop the per-draft dropdown overrides too. A discarded draft is gone, and a
  // stray entry would seed a future draft that happens to reuse the id.
  clearComposeSelection(threadId);
  lastSyncedImageHashes.delete(threadId);
  // Pairs with the push in ensureFocusedComposeThread. Back and Forward must
  // not restore a discarded thread whose events would 404.
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

/** Send the focused thread's current compose contents as the first message.
 *  Reads text/images from the draft signal so the caller doesn't need to
 *  pass them. Optimistic local clear + state→active before the chat POST so
 *  the input is immediately usable for follow-up text. On failure we restore
 *  the typed text — losing it would be the worst possible UX. */
export async function sendCompose(
  threadId: string,
  opts: { useCodingAgent?: boolean; context?: ChatContext | null; focus?: boolean },
): Promise<void> {
  // Both guards below end a send the user asked for, so neither may be silent.
  // A dispatched send that produces no message and no word is a dead button, and
  // this is the path a compose-view Send takes. See
  // docs/plans/2026-08-27-the-composer-sends-the-draft-it-is-showing.md.
  const thread = threadMap.value.get(threadId);
  if (!thread) {
    showToast('Could not send: that draft is no longer open.', 'error');
    return;
  }
  const draft = getDraft(threadId);
  const text = draft.text;
  const wireHashes = draft.image_hashes;
  const mode = draft.mode;
  if (!text.trim() && wireHashes.length === 0) {
    showToast('Could not send: the saved draft was empty.', 'error');
    return;
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
    // The chat POST needs the thread row to exist server-side, and on a
    // first-send `POST /threads` may still be in flight.
    // `ensureFocusedComposeThread` fires it without awaiting. Every optimistic
    // local step above stays synchronous, since the input must clear on the
    // gesture. So this sits as late as possible, before the network call.
    //
    // A button that composes and sends in one gesture is the case that needs
    // it. Typing hides the race, the draft PUT awaiting the same promise in
    // `pushNow`, and `cancelPendingPush` above has just dropped that PUT. A
    // failed start rejects here and lands in the catch below, which rolls the
    // draft back and rethrows for the caller to toast.
    await awaitThreadStarted(threadId);
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
    // A compose-view pick is a one-shot intent, now carried into this spawn's
    // chat body, so consume the draft's selection. Without this the override
    // would linger in `composeSelections` for a thread no longer composing.
    // Follow-ups do not come through here: an active thread has no entry.
    clearComposeSelection(threadId);
    // Scheduled here, AFTER the send resolved and the selection was consumed,
    // for two reasons. `cancelPendingPush` above dropped the debounced write,
    // and an in-flight write cannot be recalled. Without this the engine's last
    // word could be the pre-send text. And ordering it after
    // `clearComposeSelection` keeps the write from carrying the draft's picks
    // back onto a row whose `compose_selection` the projection just set to
    // NULL. A send that FAILS schedules nothing: it consumed no draft, so there
    // is no stale write to out-order, and the restored text must stay put.
    pushClearedComposeAfterSend(threadId);
  } catch (err) {
    // Roll back state. Restore text and images only if the user has not started
    // typing into the now-empty textarea. Overwriting fresh keystrokes would
    // lose work the user can see they typed.
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
  lastSyncedImageHashes.delete(threadId);
  await sendMessage(text, imageHashes, { ...opts, threadId, focus: opts?.focus ?? true });
}

/** Tab-close-safe flush. Each pending PUT goes out with `keepalive: true`, so
 *  the browser keeps the connection alive after the document tears down.
 *
 *  **The device-id header is critical.** Without it the server's broadcast
 *  carries origin_device_id=None, and other tabs cannot suppress the echo. That
 *  can clobber newer text typed elsewhere right after the close. */
function flushAllPending(): void {
  const headers: Record<string, string> = { 'Content-Type': 'application/json' };
  const deviceId = typeof localStorage !== 'undefined' ? localStorage.getItem('lucidos-device-id') : null;
  if (deviceId) headers['x-lucidos-device-id'] = deviceId;

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
      compose_epoch: composeEpoch.get(threadId),
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
  // iOS bfcache can resume the page with frozen JS state; without this, stale
  // entries would block the next loadAllThreads from refreshing those threads.
  // `owedComposePushes` goes with them: its intents have just been flushed, and
  // a frozen entry would make a resumed page's runner issue a redundant write.
  pendingComposePuts.clear();
  owedComposePushes.clear();
}

if (typeof window !== 'undefined') {
  window.addEventListener('beforeunload', flushAllPending);
  window.addEventListener('pagehide', flushAllPending);
}
