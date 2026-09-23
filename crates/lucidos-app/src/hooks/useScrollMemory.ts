import { useEffect, useRef } from 'preact/hooks';
import type { RefObject } from 'preact';

import {
  deepLinkHasResolved,
  EVENT_RESOLVE_DEADLINE_MS,
  followPosition,
  hasPendingEventScroll,
  markNavigationScroll,
  onDeepLinkClaimed,
  onDeepLinkResolved,
  onFollowArmed,
  resumeFollowingBottom,
  applyFollowSeed,
  type FollowResumeFrom,
} from '../components/chat/scrollState';
import { ANCHOR_ATTR, anchorTargetTop, anchorTurnIsClamped, readScrollAnchor, type ScrollAnchor } from '../components/chat/scrollAnchor';
import { onPageHide, onPageWake } from '../utils/pageVisit';
import { watchUserAction } from '../utils/userAction';

/** True iff the container's current measurement can hold the saved offset
 *  (i.e. content has grown enough). Used to gate ResizeObserver-driven
 *  restore retries while async content is still rendering.
 *  saved=0 (user was at the top) is always restorable, and telling "user
 *  scrolled to top" apart from "no save" matters: the second opens at the top
 *  via `resetOnEmpty`, the first is a position the reader chose. */
export function isFullyRestorable(saved: number, scrollHeight: number, clientHeight: number): boolean {
  if (saved < 0) return false;
  if (saved === 0) return true;
  const max = Math.max(0, scrollHeight - clientHeight);
  return max >= saved;
}

/** The stored form of a reading position that is the LIVE EDGE rather than a
 *  pixel offset: the reader had a standing follow armed in this thread, and was
 *  ON the edge, when they left it. Deliberately not a number, and deliberately
 *  not a second key beside the offset: a thread opens in exactly one place, so
 *  the two answers share one slot and cannot disagree. */
export const LIVE_EDGE_VALUE = 'live-edge';

/** The stored marker for a reader who holds a standing follow while parked
 *  somewhere OTHER than the live edge: `following:` in front of either place
 *  form. It is a REQUEST rather than a place, so it sits beside one instead of
 *  replacing it.
 *
 *  ARMED and CARRYING are two states (ADR 0064), and this is the first without
 *  the second. A ride carries its reader only while the thread is running, so
 *  re-entry puts them back on the turn they parked on and re-arms. The growth
 *  branch takes them to the edge once there is something to be carried toward.
 *
 *  The alternative, recording the place OR the request, is what this replaced:
 *  every clamp threw the ride away silently, with the toggle still lit. See
 *  `docs/plans/2026-09-16-the-record-follows-the-request.md`. */
const FOLLOWING_PREFIX = 'following:';

/** The stored form of a reading position that names a TURN:
 *  `anchor:<relTop>:<eventId>`. Why a turn and not a pixel offset, and every
 *  alternative weighed: ADR 0152, docs/adr/.
 *
 *  `relTop` leads, so an id carrying a colon survives the round trip: the parse
 *  splits on the FIRST one. It is EXACT rather than a fallback to "somewhere
 *  near", `relTop` being the pixel offset the anchored turn's top sat at. */
const ANCHOR_PREFIX = 'anchor:';

/** Where a thread opens. A TURN the reader was parked on (`ANCHOR_PREFIX`), the
 *  live edge they asked to ride (`LIVE_EDGE_VALUE`), or a bare pixel offset.
 *  `null` for no saved position at all, which `resetOnEmpty` turns into the top.
 *
 *  The offset is what every container that is NOT the transcript records, none
 *  of them being windowed. It is also what a transcript position written by an
 *  older build still reads as.
 *
 *  `armed` rides ALONGSIDE the two place forms and says the reader holds a
 *  standing follow there (`FOLLOWING_PREFIX`). Absent means they do not, which
 *  is the honest reading of a stored value carrying no marker. `live-edge` is
 *  armed by definition and needs no field. */
export type SavedScroll =
  | { kind: 'offset'; top: number; armed?: true }
  | { kind: 'live-edge' }
  | ({ kind: 'anchor'; armed?: true } & ScrollAnchor);

/** The stored form of an anchor, the one writer of `ANCHOR_PREFIX`. */
function formatScrollAnchor(anchor: ScrollAnchor): string {
  return `${ANCHOR_PREFIX}${Math.round(anchor.relTop)}:${anchor.eventId}`;
}

/** Put the armed marker in front of a place, the one writer of
 *  `FOLLOWING_PREFIX`. */
function formatArmedPlace(place: string): string {
  return `${FOLLOWING_PREFIX}${place}`;
}

/** Parse a localStorage scroll value. Returns null on missing, invalid or
 *  negative input.
 *
 *  Tolerates a trailing `:<revision>` stamp that older stored positions carry.
 *  `parseFloat` reads the offset out and ignores the rest, so a browser holding
 *  one keeps its position. An `anchor:` value is read first, so that tolerance
 *  never sees one.
 *
 *  A build predating either sentinel reads it as junk and answers null, so a
 *  downgrade opens the thread at the top rather than somewhere wrong. No
 *  migration either way: all that is at stake is where one thread opens once. */
export function parseSavedScroll(raw: string | null): SavedScroll | null {
  if (raw === null || raw === '') return null;
  if (raw === LIVE_EDGE_VALUE) return { kind: 'live-edge' };
  // The armed marker comes off FIRST, and what is left is parsed exactly as an
  // unmarked value is. So the two forms cannot drift, and a stored value that
  // carries no marker reaches the same code it always did.
  if (raw.startsWith(FOLLOWING_PREFIX)) {
    const place = parseSavedScroll(raw.slice(FOLLOWING_PREFIX.length));
    // A marker in front of nothing legible is not a position, and it is not a
    // request either: the request is about the place it qualifies. A marked
    // `live-edge` is a value we never write, and says the same thing twice.
    return place === null || place.kind === 'live-edge' ? place : { ...place, armed: true };
  }
  if (raw.startsWith(ANCHOR_PREFIX)) return parseAnchor(raw.slice(ANCHOR_PREFIX.length));
  const n = Number.parseFloat(raw);
  if (!Number.isFinite(n) || n < 0) return null;
  return { kind: 'offset', top: Math.floor(n) };
}

/** `<relTop>:<eventId>`, split on the FIRST colon so an id may carry one.
 *
 *  `relTop` is matched whole rather than handed to `parseInt`, which reads
 *  `12abc` as 12. A malformed value is one we did not write, and guessing at it
 *  would put the reader somewhere nobody asked for. It is signed: the ordinary
 *  anchor sits at or above the viewport top. */
function parseAnchor(rest: string): SavedScroll | null {
  const sep = rest.indexOf(':');
  if (sep < 1) return null;
  const relTop = rest.slice(0, sep);
  const eventId = rest.slice(sep + 1);
  if (!/^-?\d+$/.test(relTop) || eventId === '') return null;
  return { kind: 'anchor', eventId, relTop: Number(relTop) };
}

/** What is RECORDED under `key`, or null for nothing legible.
 *
 *  Storage can THROW rather than answer null, in a browser with it blocked, so
 *  every read in the app goes through this. Exported because ThreadView asks
 *  the same question of the same keys. It has to render the anchored turn
 *  before this hook can put the reader on it. */
export function readSavedScroll(key: string): SavedScroll | null {
  try {
    return parseSavedScroll(localStorage.getItem(key));
  } catch {
    return null;
  }
}

/** localStorage key for a chat thread's saved scroll offset. */
export function threadScrollKey(threadId: string): string {
  return `lucidos-scroll-thread-${threadId}`;
}


/** localStorage key for ContentPane's per-view scroll offset. ContentPane
 *  reads/writes this directly; the prefix lives here so resetContentScroll
 *  stays in sync. */
export function contentScrollKey(viewKey: string): string {
  return `lucidos-scroll-content-${viewKey}`;
}

/** Drop a ContentPane view's saved scroll so the next mount lands at the top
 *  instead of restoring (e.g., after saving a form). */
export function resetContentScroll(viewKey: string): void {
  try {
    localStorage.removeItem(contentScrollKey(viewKey));
  } catch { /* quota or disabled, ignore */ }
}

const SAVE_DEBOUNCE_MS = 150;
/** Longest a CONTENT change waits before the place is re-read.
 *
 *  A throttle rather than a debounce, so a transcript that mutates without
 *  pause still commits. A streaming turn mutates on every token, and a debounce
 *  there defers the read until the stream stops.
 *
 *  It exists because the read is not free: `readScrollAnchor` measures rects,
 *  and a measurement per mutation batch is a forced layout per batch. One per
 *  window bounds that, and the reading position is not urgent. */
const CONTENT_READ_THROTTLE_MS = 150;
/** How long to wait for async content to render before giving up on restoring.
 *  Long enough for typical Loadable<T> roundtrips, short enough that a stuck
 *  observer does not permanently suppress saves.
 *
 *  **Giving up MOVES NOBODY.** Clamping an unreachable offset to the current
 *  maximum is the live edge and nothing else, which nothing may scroll to on
 *  its own (ADR 0064). `scrollToSelectorAndPulse`'s deadline reaches the same
 *  conclusion for a dead deep-link.
 *
 *  Only a container nobody WINDOWS records an offset, so its content settles
 *  at a height three seconds either finds or does not. The transcript names a
 *  turn instead (`ANCHOR_PREFIX`), and waits differently: see
 *  `ANCHOR_RESTORE_CEILING_MS`. */
const RESTORE_DEADLINE_MS = 3000;
/** Longest an ANCHOR restore may keep re-arming the deadline above while the
 *  transcript is still growing toward the turn it names.
 *
 *  Three seconds is not the whole budget for an anchor. ThreadView walks its
 *  render window up to a deep one across many commits, rather than in one
 *  blocking render. Giving up part-way through that walk would leave the reader
 *  short of where they were, which is the approximation this form refuses.
 *
 *  A ceiling all the same, so a container that grows forever cannot hold the
 *  observers and suppress every save for the life of the thread. */
const ANCHOR_RESTORE_CEILING_MS = 20_000;
/** Grace on top of `EVENT_RESOLVE_DEADLINE_MS` before a stood-down open decides
 *  the deep-link is dead and positions the thread itself. Covers a release
 *  landing a beat after its own deadline. Short enough that a dead link is not
 *  left on a borrowed offset much longer than the toast.
 *
 *  It is a grace on the link's budget, and that budget is ELASTIC: the link
 *  re-arms its own deadline while the thread's events are still arriving. So
 *  the rescue re-checks rather than firing once, and waits out a claim still
 *  held. See `hasPendingEventScroll` at the timer below. */
const DEAD_DEEP_LINK_SLACK_MS = 500;

export interface ScrollMemoryOptions {
  /** When true, don't restore even if a saved value exists. Useful while
   *  content is still loading. */
  paused?: boolean;
  /** Predicate called once at restore time (effect setup). Return false to SKIP
   *  RESTORING a saved offset for this mount or key change, while still
   *  attaching the save listener. Defaults to always-restore. Chat passes
   *  `() => !hasPendingEventScroll()`, so a notification deep-link resolving a
   *  scroll to a specific event is not overridden.
   *
   *  **It gates BOTH the restore and the `resetOnEmpty` reset**, since both are
   *  this hook placing the reader and the deep-link's landing must win. The
   *  attach cannot be assumed to happen before that landing. Under reduced
   *  motion the landing is one synchronous write with nothing to re-assert it,
   *  so an ungated reset simply overwrote it.
   *
   *  Standing down leaves a hole this hook closes itself: if the deep-link
   *  never lands, nothing else ever positions the thread. See the
   *  `!allowRestore` branch in `attachScrollMemory`. */
  shouldRestore?: () => boolean;
  /** When true, this is a SHARED scroll container. The previous view's offset
   *  persists on the DOM, so the hook writes `scrollTop = 0` itself wherever it
   *  has no position to put the reader in. Two such places, and they are the
   *  whole of what this gates: the no-save case, and the WAIT for a saved
   *  offset the content is not yet tall enough to hold. `saved === 0` is
   *  neither, being a real restore that always writes scrollTop. Off by
   *  default.
   *
   *  For the transcript this is what makes a thread with no saved position open
   *  at the top, the way a document opens. */
  resetOnEmpty?: boolean;
  /** When true, this container's saved position may be the LIVE EDGE as well as
   *  an offset. An armed standing follow is recorded here and resumed on
   *  re-entry, so a reader watching an agent work still is after visiting
   *  another thread. Off by default.
   *
   *  An opt-in rather than the default, because the follow is one global while
   *  the three containers this hook serves are not. Without the gate, arming a
   *  follow in the transcript would stamp the live edge onto whatever the
   *  content pane or the thread drawer was showing. Only the transcript can
   *  ride a live edge, so only the transcript records one. */
  followsLiveEdge?: boolean;
  /** When true, this container records WHICH TURN the reader is on rather than
   *  a pixel offset (see `ANCHOR_PREFIX`). Off by default.
   *
   *  An opt-in for the same reason `followsLiveEdge` is, and the two mark out
   *  the same container. Only the transcript is WINDOWED, so only there does a
   *  pixel offset stop meaning anything between opens. And only the transcript
   *  has children that can be named: the content pane and the thread drawer
   *  stamp no id on theirs, so an anchor there could not be resolved back. */
  anchorsToContent?: boolean;
  /** Called once this attachment will no longer place the reader: it landed
   *  them, it gave up, a deep-link took the open, or the reader took over.
   *
   *  It exists for work done ON BEHALF of a restore that has to stop with it.
   *  ThreadView walks its render window up to an anchored turn, and that walk
   *  renders markdown a round at a time. Left running past the restore, it pays
   *  the whole cost of a landing nobody will make. */
  onRestoreSettled?: () => void;
}

/** The options an attachment reads LIVE, whose current value belongs to
 *  whatever the component last rendered rather than to the attachment. The hook
 *  hands them over as one getter so the attachment cannot capture them at setup
 *  and go stale. The flip side: reading one after the attachment stopped being
 *  current reads the NEXT thing's value. See `observed` below. */
export type ScrollMemoryLive = Pick<ScrollMemoryOptions, 'shouldRestore' | 'onRestoreSettled'>;

/** Wire one scroll container to one storage key: restore on attach, persist on
 *  scroll, flush on teardown. Returns the teardown.
 *
 *  Extracted from the hook, a thin `useEffect` over it, so the whole lifecycle
 *  including teardown is drivable from a test with a fake element. The
 *  teardown's correctness depends on WHEN each value was read, which no
 *  assertion over the hook could reach.
 *
 *  Save is debounced by ~150ms to avoid storage thrash during scroll. */
export function attachScrollMemory(
  el: HTMLElement,
  key: string,
  opts: {
    live: () => ScrollMemoryLive;
    resetOnEmpty?: boolean;
    followsLiveEdge?: boolean;
    anchorsToContent?: boolean;
    isCurrent?: () => boolean;
  },
): () => void {
  const {
    live,
    resetOnEmpty = false,
    followsLiveEdge = false,
    anchorsToContent = false,
    isCurrent,
  } = opts;

  let saveTimer: ReturnType<typeof setTimeout> | null = null;
  // hasWritten is the dedup gate. lastSaved alone cannot tell "wrote 0" from
  // "wrote nothing yet". The first writeNow must act, so that a stale
  // localStorage value from a previous session is reconciled.
  let lastSaved: string | null = null;
  let hasWritten = false;
  let restoring = true;
  /** Whether `settleRestore` has already spoken. */
  let hasSettled = false;
  /** The value to commit, captured WHEN THE SCROLL HAPPENED rather than when
   *  the debounce fires. The container's offset belongs to THIS key, and stops
   *  belonging to it at teardown. The teardown runs from the hook's effect
   *  cleanup, which Preact defers past the render that changed `key`. By then
   *  the shared `.thread-content` shows the INCOMING thread.
   *
   *  Reading it there would write the outgoing thread's key with the incoming
   *  thread's offset. Snapshotting leaves nothing at teardown for the new
   *  render to have moved, making that unrepresentable rather than merely
   *  fixed.
   *
   *  `undefined` means this key has seen no scroll, which is what makes an
   *  unreached `writeNow` do nothing rather than delete a stored position. */
  let observed: string | undefined;
  /** Watches the CONTENT for a change that moves the reading position without
   *  moving the container, and the throttle that bounds the re-read. Only an
   *  anchoring container arms them: see `watchContent`. */
  let contentObserver: MutationObserver | null = null;
  let contentTimer: ReturnType<typeof setTimeout> | null = null;
  let resizeObserver: ResizeObserver | null = null;
  let mutationObserver: MutationObserver | null = null;
  let deadlineTimer: ReturnType<typeof setTimeout> | null = null;
  /** Teardown for the user-action watch that runs FOR the wait, and `null`
   *  whenever no wait is armed.
   *
   *  The wait can run for three seconds, long enough for the reader to have
   *  settled in. Landing a three-second-old record on top of them then is the
   *  app moving them. So the first thing they do retires the restore: the
   *  position they are at is theirs, and the record is only an offer.
   *
   *  Asked as a GESTURE (`watchUserAction`) and never as a change in
   *  `scrollTop`. A pixel delta cannot tell the reader from the app, and the app
   *  writes `scrollTop` all through this window without going through
   *  `markNavigationScroll`. Reading any of those writes as a gesture would
   *  abandon the reader's saved position for good. None of them emits an input
   *  event, which is what `watchUserAction` documents itself for. */
  let stopUserWatch: (() => void) | null = null;
  /** Where the container sat when the FIRST deep-link of this open took it, and
   *  `null` while none has. The dead-link rescue's reference point. See
   *  `standDownForDeepLink` for why it is captured once rather than per claim. */
  let inheritedBeforeDeepLink: number | null = null;
  /** True while THIS attachment is replaying an arm it read off the record, so
   *  the arm subscription can tell a replay from a new request. See
   *  `replayParkedArm`. */
  let replayingArm = false;
  /** When this attachment started waiting, and how tall the container was at
   *  its last look. The two terms `keepWaitingForAnchor` extends on: elapsed
   *  against the ceiling, and a height that has changed since. */
  const restoreStartedAt = Date.now();
  let lastHeight = el.scrollHeight;

  /** What is RECORDED for this key right now. Re-readable rather than read
   *  once: the attach-time answer goes stale the moment the reader scrolls, and
   *  the wake below must not act on a snapshot from before they did. */
  const readSaved = (): SavedScroll | null => {
    const value = readSavedScroll(key);
    // Only a container that can RECORD a form can restore it. Anything else
    // reading one is reading a value it did not write, and the honest answer is
    // then no saved position: not a bottom nobody asked for, and not a turn
    // whose id means nothing among these children.
    if (value?.kind === 'live-edge' && !followsLiveEdge) return null;
    if (value?.kind === 'anchor' && !anchorsToContent) return null;
    // The armed marker is the one part that is DROPPED rather than rejected
    // with its value. It qualifies a place this container can still honour, and
    // the follow is one global: arming from here would hand the transcript's
    // request to whatever the content pane was showing.
    if (value !== null && value.kind !== 'live-edge' && value.armed && !followsLiveEdge) {
      return { ...value, armed: undefined };
    }
    return value;
  };

  /** What was recorded when this attachment was set up, which is what decides
   *  the branch it takes below. Anything running LATER re-reads instead: an arm
   *  this open makes changes the answer under it. */
  const saved: SavedScroll | null = readSaved();

  /** Say ONCE that nothing will place the reader from this thread's record any
   *  more, so work done on behalf of the restore can stop (`onRestoreSettled`).
   *
   *  Separate from `stopRestore` because two of its callers are not that.
   *  Handing the open to a deep-link keeps the rescue, which places the reader
   *  from the same record. A teardown can be a re-attach under the same key,
   *  and then the next attachment owns the question.
   *
   *  Guarded like every other write: a superseded attachment tears down after
   *  the render that changed the key, and the work it would stop belongs to the
   *  next thread. */
  const settleRestore = () => {
    if (hasSettled) return;
    hasSettled = true;
    if (!isCurrent || isCurrent()) live().onRestoreSettled?.();
  };

  const stopRestore = (settled = true) => {
    restoring = false;
    resizeObserver?.disconnect();
    resizeObserver = null;
    mutationObserver?.disconnect();
    mutationObserver = null;
    if (deadlineTimer !== null) {
      clearTimeout(deadlineTimer);
      deadlineTimer = null;
    }
    stopUserWatch?.();
    stopUserWatch = null;
    if (settled) settleRestore();
  };

  /** Is this open OURS to position, or does something higher-priority own it?
   *  One question, asked at each of the four moments this attachment is about
   *  to place the reader: at attach (`allowRestore`), from the dead-deep-link
   *  rescue, on a page wake, and when a deep-link announces a claim. */
  const openIsOurs = () => live().shouldRestore?.() ?? true;

  /** Pick a recorded standing follow back up for a reader who parked somewhere
   *  other than the live edge.
   *
   *  ALWAYS GUARDED, whichever resume it uses. `onFollowArmed` exists to write
   *  a NEW request down, so it retires the restore and stamps the live edge on
   *  the record. Both are wrong for a REPLAY of a parked reader. The request is
   *  already on disk, and the live edge is not where they were. The restore
   *  this travels with is what puts them back on their turn. */
  const replayParkedArm = (from: FollowResumeFrom) => {
    replayingArm = true;
    try {
      resumeFollowingBottom(el, from);
    } finally {
      replayingArm = false;
    }
  };

  /** Resume whatever standing follow `record` holds, for a site that is NOT
   *  positioning the reader itself.
   *
   *  `live-edge` is its own place, so that form writes today's bottom. A parked
   *  one arms alone, the ride carrying such a reader only once the thread runs.
   *
   *  A DEEP LINK owning the open (`in-place`) overrides that for both forms,
   *  and a parked record needs it as much as the edge form does. A link landing
   *  off the live edge ends the ride on purpose. Replaying it would light the
   *  toggle back up over the event the link exists to show. That branch holds
   *  the three guards deciding it. */
  const replayRecordedFollow = (record: SavedScroll | null, edgeFrom: FollowResumeFrom) => {
    if (record?.kind === 'live-edge') resumeFollowingBottom(el, edgeFrom);
    else if (record?.armed) replayParkedArm(edgeFrom === 'in-place' ? 'in-place' : 'parked');
  };

  /** Where a RECORD says this container should sit RIGHT NOW, or null while it
   *  cannot be honoured yet. One expression, so the restore retry and the
   *  dead-link rescue cannot read the same record two different ways.
   *
   *  **REACHABILITY IS ASKED OF BOTH FORMS, BEFORE ANY WRITE.** An offset needs
   *  the content tall enough to hold it. An anchor needs that too, plus the turn
   *  it names in the DOM. On a windowed transcript the second waits for
   *  ThreadView to walk its render window up to that turn.
   *
   *  Detecting a browser CLAMP after the fact is not the same test, and must not
   *  stand in for it. An offset that overshoots says only that the reader sat
   *  where this content cannot reach. Clamping it invents a position: the live
   *  edge, which nothing may scroll to on its own (ADR 0064).
   *
   *  A RESOLVED anchor is the opposite case, and lands at once. The turn was
   *  found and measured, so the nearest reachable offset still shows it.
   *
   *  Waiting cannot bring it closer. The window renders from its edge to the
   *  newest turn, so everything below a rendered turn is already drawn. A
   *  shortfall means that content got shorter since the save. See ADR 0152
   *  (docs/adr/).
   *
   *  A turn drawn with its head CLAMPED is not resolved yet. Its rows are still
   *  arriving, and landing on it would settle the restore and stop the walk. */
  const offsetFor = (record: SavedScroll): number | null => {
    const top = record.kind === 'anchor'
      ? (anchorTurnIsClamped(el, record) ? null : anchorTargetTop(el, record))
      : record.kind === 'offset' ? record.top
      : null;
    if (top === null) return null;
    if (isFullyRestorable(top, el.scrollHeight, el.clientHeight)) return top;
    if (record.kind !== 'anchor') return null;
    return Math.max(0, el.scrollHeight - el.clientHeight);
  };

  /** Put the reader where the record's PLACE says, whatever request rides
   *  beside it. One expression, so an armed record and a bare one cannot open
   *  in two different places.
   *
   *  A turn lands NOW when it is already rendered, which is every revisit
   *  inside the window the thread opens at. Otherwise the observers below wait
   *  for ThreadView to walk the window up to it. An offset lands now when the
   *  transcript is already tall enough to hold it, the ordinary revisit.
   *  Deferring even to the next frame would paint the borrowed offset once on
   *  the way. It would also open a window in which a gesture retires a wait
   *  that never needed to happen.
   *
   *  A zero is written explicitly rather than waited for, for the same
   *  shared-container reason the no-record branch writes one. */
  const positionFromRecord = () => {
    if (saved === null || saved.kind === 'live-edge') return;
    if (saved.kind === 'offset' && saved.top === 0) {
      markNavigationScroll(el, 0);
      restoring = false;
      return;
    }
    tryRestore();
  };

  /** `final` says the restore window has closed, so no further round is
   *  armed. */
  const tryRestore = (final = false) => {
    if (!restoring || saved === null || saved.kind === 'live-edge') return;
    const top = offsetFor(saved);
    if (top === null) {
      if (!final) keepWaitingForAnchor();
      return;
    }
    markNavigationScroll(el, top);
    stopRestore();
  };

  /** Push the restore deadline out while an ANCHOR's transcript is still
   *  CHANGING under it. Bounded by `ANCHOR_RESTORE_CEILING_MS`, and by
   *  progress: a settled container is left to expire on the deadline it has.
   *
   *  Progress is any CHANGE in height, never a new high-water mark. The walk
   *  prepends turns, so the trend is upward, but the transcript shrinks under it
   *  too. A live Thinking row folding into its summary is the ordinary case.
   *  Measured against a maximum, every round after such a shrink reads as no
   *  progress, and the reader is abandoned mid-walk.
   *
   *  Only the anchor form asks for this. An offset is reachable at a height the
   *  transcript either finds in three seconds or does not, and no walk changes
   *  that. */
  /** Has this attachment spent the whole anchor budget? The one bound on both
   *  ways the wait extends, so neither can outlive it. */
  const ceilingReached = () => Date.now() - restoreStartedAt >= ANCHOR_RESTORE_CEILING_MS;

  const keepWaitingForAnchor = () => {
    if (saved?.kind !== 'anchor' || deadlineTimer === null) return;
    if (el.scrollHeight === lastHeight) return;
    lastHeight = el.scrollHeight;
    if (ceilingReached()) return;
    clearTimeout(deadlineTimer);
    deadlineTimer = setTimeout(onDeadline, RESTORE_DEADLINE_MS);
  };

  /** The restore window closing: one last look, then stop. No fallback
   *  position, and never a clamp to whatever the content currently allows (see
   *  `RESTORE_DEADLINE_MS`).
   *
   *  The last look is not a formality. Two ways the container grows are
   *  invisible to both observers. An image or a font decoding changes
   *  `scrollHeight` without mutating the DOM. The container's own box never
   *  changes, being a flex child of a fixed parent. A now-reachable offset would
   *  otherwise be dropped for want of a callback.
   *
   *  Nothing gates it here. A reader who has taken over retired the whole wait
   *  when they did (see `stopUserWatch`), so this timer no longer exists for
   *  them. */
  const onDeadline = () => {
    // A container with NO BOX cannot be placed in, and cannot report progress
    // either: an unmeasured height never changes, so `keepWaitingForAnchor` is
    // silent for it. A collapsed desktop split gives one, and the reader is not
    // reading through it. Wait for the pane instead of spending the last look
    // on a measurement that means nothing, bounded by the same ceiling.
    if (anchorsToContent && el.clientHeight <= 0 && !ceilingReached()) {
      deadlineTimer = setTimeout(onDeadline, RESTORE_DEADLINE_MS);
      return;
    }
    tryRestore(true);
    stopRestore();
  };

  /** Hand this open to a deep-link: stop positioning the reader ourselves, and
   *  arm the rescue that covers the link turning out DEAD. **Both halves,
   *  always.** One function serves the two places a deep-link can take this
   *  open. A site doing only the first half leaves exactly the hole the rescue
   *  exists to close.
   *
   *  Whatever the link lands on is where the reader asked to be, so positioning
   *  here would overwrite it. A DEAD link positions nothing, and
   *  `.thread-content` is one element reused across threads. It keeps showing
   *  the OUTGOING thread's offset, which the save listener then persists as
   *  this thread's remembered position.
   *
   *  So wait out the link's own budget and then position, but ONLY if the
   *  container has not moved a pixel meanwhile. That is exactly "the landing
   *  never happened": a landing moves it, and so does the reader.
   *
   *  Re-entrant, a second notification tapped mid-window being a second claim.
   *  A link SUPERSEDED by a newer claim still lands, yet neither announces nor
   *  latches its resolve, so a dead second link can position over it. */
  const standDownForDeepLink = () => {
    // Also retires the restore observers, which is a no-op at attach (none are
    // armed yet) and the whole point from the claim broadcast. Clears a rescue
    // already in flight too, which is what the re-arm below replaces.
    //
    // NOT settled: the rescue below places the reader from this same record, so
    // work done on its behalf must outlive the hand-over. Every way the rescue
    // can end says so itself.
    stopRestore(false);
    // **The link owns the POSITION on this open, not the REQUEST.** Standing
    // down means do not place the reader. Every branch below places them, so a
    // deep-linked open is the one open that never reaches the resume. The
    // request is resumed here instead, `in-place` so nothing is written over
    // the landing. `resumeFollowingBottom` declines once the link has landed
    // OFF the live edge, that landing having ended the ride on purpose. So the
    // ride is held open for a link still in flight, and for one that came to
    // rest where the ride was heading anyway. A dead link costs nothing either.
    //
    // Kept ahead of the early return below, even though a landed link declines
    // inside the resume. Both orderings must reach the same place, and one
    // guard makes that true by construction rather than by reading both paths.
    //
    // The record is RE-READ rather than taken from the attach-time snapshot: a
    // claim broadcast can arrive long after the reader's own scroll changed the
    // answer. Gated on `followsLiveEdge` like every other live-edge branch, so
    // the content pane and the thread drawer cannot arm the transcript's
    // follow.
    if (followsLiveEdge) {
      const recorded = readSaved();
      // No record at all is the one case the *follow seed* speaks for, and a
      // deep link does not change that this thread has none.
      if (recorded === null) applyFollowSeed(el, 'in-place');
      else replayRecordedFollow(recorded, 'in-place');
    }
    // A link that has ALREADY found its target is positioning the reader, so
    // there is no dead link to rescue. ASKED rather than waited for: the
    // resolve broadcast reaches only listeners that exist when it fires, and
    // the ordinary tap resolves before Preact runs the effect that attaches
    // this. The rescue's own "has anything moved" test cannot stand in for the
    // question, a landing with nowhere to move looking exactly like a dead
    // link.
    //
    // The landing is also this thread's reading position, and this attachment
    // missed the announcement that said so, so it records it here instead. Same
    // pairing as the subscription below, reached from the other side.
    if (deepLinkHasResolved()) {
      recordDeepLinkLanding();
      settleRestore();
      return;
    }
    // Captured from the FIRST deep-link of this open and never re-read, which
    // is what makes re-arming safe. A first link that LANDED moved the
    // container. Re-reading here would make its landing the new reference
    // point. A dead SECOND link would then rescue the reader away from the
    // event the first one took them to. Held against the original, that case
    // reads as "something positioned this thread" and the rescue declines.
    if (inheritedBeforeDeepLink === null) inheritedBeforeDeepLink = el.scrollTop;
    const inherited = inheritedBeforeDeepLink;
    const armDeadline = () => {
      deadlineTimer = setTimeout(() => {
        deadlineTimer = null;
        // WAIT, rather than decline, while the link still holds its claim. Its
        // own deadline re-arms while the thread's events arrive, so a fixed
        // round is no longer the whole of its budget. The claim releases
        // whichever way the link ends, and a resolve cancels this rescue
        // outright (`onDeepLinkResolved` below), so this terminates.
        //
        // It comes BEFORE `openIsOurs`, which is the same question read the
        // other way round (ThreadView's `shouldRestore`). Through that guard a
        // slow thread reads as somebody else's open, so the rescue drops
        // instead of deferring. A dead link then keeps the outgoing offset.
        if (hasPendingEventScroll()) {
          armDeadline();
          return;
        }
        // Every path from here ends the rescue, and the rescue is the last thing
        // that places the reader from this record. So this is where the
        // hand-over at the stand-down finally settles.
        settleRestore();
        // Same question `onScroll` asks, for the same reason. The teardown is
        // deferred past the render that changed `key`. A superseded attachment
        // must not position a container that now belongs to the next thread.
        if (isCurrent && !isCurrent()) return;
        if (el.scrollTop !== inherited) return;
        if (!openIsOurs()) return; // a newer deep-link owns it now
        // RE-READ, exactly as the stand-down above does, and for a sharper reason.
        // The stand-down can ARM this open through the *follow seed*, on a thread
        // whose record was empty when this attachment read it. That arm records
        // the live edge, so the attach-time snapshot no longer describes the
        // thread. Read through it, the rescue takes its reset branch below and
        // hauls an armed reader to the top.
        //
        // The arm's save is debounced by `SAVE_DEBOUNCE_MS`, and this timer is
        // `EVENT_RESOLVE_DEADLINE_MS` plus its slack. The first is two orders of
        // magnitude shorter, so the arm is committed by the time this reads.
        const recorded = readSaved();
        const rescueTop = recorded === null ? null : offsetFor(recorded);
        if (recorded?.kind === 'live-edge') {
          resumeFollowingBottom(el);
        } else if (rescueTop !== null) {
          markNavigationScroll(el, rescueTop);
        } else if (resetOnEmpty) {
          // Either there was no position, or there is one the content cannot
          // hold. Both open the thread where a thread with no position opens, at
          // the top of what is rendered. Never clamp an unreachable offset to the
          // container's maximum: that is the live edge (see
          // `RESTORE_DEADLINE_MS`). A container that is not shared writes nothing
          // at all, having no borrowed offset for the rescue to displace.
          markNavigationScroll(el, 0);
        }
      }, EVENT_RESOLVE_DEADLINE_MS + DEAD_DEEP_LINK_SLACK_MS);
    };
    armDeadline();
  };

  const writeNow = () => {
    const next = observed;
    if (next === undefined) return; // no scroll seen under this key
    if (hasWritten && next === lastSaved) return;
    lastSaved = next;
    hasWritten = true;
    try {
      localStorage.setItem(key, next);
    } catch { /* quota or disabled, ignore */ }
  };

  const scheduleSave = () => {
    if (saveTimer !== null) clearTimeout(saveTimer);
    saveTimer = setTimeout(writeNow, SAVE_DEBOUNCE_MS);
  };

  /** WHERE the reader is parked, in the form this container can record. Null
   *  for nothing worth recording, which only an anchoring container can answer.
   *
   *  The ANCHOR leads, on a container that can name its children. The offset is
   *  its fallback rather than a second choice of policy: an empty transcript
   *  has no turn to name, and the number is still the honest answer for a
   *  container that is not windowed. */
  const currentPlace = (): string | null => {
    if (anchorsToContent) {
      const anchor = readScrollAnchor(el);
      if (anchor) return formatScrollAnchor(anchor);
      // Children, yet not one of them nameable, means a container nobody can
      // MEASURE: a collapsed pane reports every rect all-zero. The offset below
      // would be the clamp that collapse produced, and writing it would replace
      // the turn the reader parked on with a number. Say nothing instead.
      //
      // The transcript always has children while this hook is attached, its
      // title row among them, so the offset below is the OTHER containers'
      // answer alone. An empty thread therefore records nothing rather than a
      // zero, which is the same place `resetOnEmpty` opens it at anyway.
      if (el.children?.length) return null;
    }
    return String(Math.floor(el.scrollTop));
  };

  /** The whole reading position RIGHT NOW: the place, and the reader's standing
   *  request if they hold one. One expression, two callers (the scroll listener
   *  and the deep-link landing below), so the two cannot disagree.
   *
   *  THE LIVE EDGE LEADS, for a rider who is on it. Every growth round writes
   *  `scrollTop`, so recording the number would overwrite the request.
   *
   *  A rider momentarily OFF it keeps the request all the same, in front of the
   *  place they are at. Three scrolls move a reader without ending the ride: a
   *  shrink clamping them down, the platform, and their own gesture on a quiet
   *  thread.
   *
   *  **The question is never the flag alone.** `.thread-content` carries two
   *  scroll listeners, the disarm in `makeScrollObservers` and the save here,
   *  so the flag would answer differently depending on which ran first.
   *  `followPosition` is order-independent, and answers in ONE call.
   *
   *  A deep-link landing needs no case of its own. It retires the ride before
   *  recording (see `stopFollowingBottom`), so a landing off the edge answers
   *  with the bare place. That is what the reader asked for. */
  const currentPosition = (): string | null => {
    const follow = followsLiveEdge ? followPosition(el) : null;
    if (follow === 'live-edge') return LIVE_EDGE_VALUE;
    const place = currentPlace();
    if (place === null) return null;
    return follow === 'parked' ? formatArmedPlace(place) : place;
  };

  /** Record where a deep-link landed as this thread's reading position.
   *
   *  Going to a link SETS the memory. The landing is a reading position like
   *  any other. Coming back to the thread returns the reader there, not to
   *  whatever they parked on before following the link.
   *
   *  The scroll listener cannot be left to notice it: two ordinary landings
   *  produce no scroll event it will see. Under reduced motion the landing is
   *  one synchronous write happening before this attachment exists. A landing
   *  with nowhere to move writes nothing at all, which is what arriving in a
   *  shorter thread and linking to its last turn does. In both the thread would
   *  keep its stale position, undoing the reader's navigation.
   *
   *  An ANIMATED landing is recorded at its start and corrected by its own
   *  frames. Each writes `scrollTop`, and each resulting scroll event pushes
   *  the debounce out again, so storage sees the settled position. */
  const recordDeepLinkLanding = () => {
    // The guard lives HERE rather than at the two call sites, because it
    // belongs to the write. A superseded attachment stays subscribed until its
    // deferred teardown, and the landing it hears is the INCOMING thread's.
    // Recording it would put that offset on the OUTGOING thread's key. Holding
    // the guard at the write means no caller can reintroduce that.
    if (isCurrent && !isCurrent()) return;
    const next = currentPosition();
    if (next === null) return;
    observed = next;
    scheduleSave();
  };

  /** Re-read the place after the CONTENT changed under a reader who did not
   *  move. The transcript is windowed and paged, so the turn at the top can
   *  change with no scroll at all. A backfill fold gives the top turn the id it
   *  was missing, and until then the record names the turn BELOW it.
   *
   *  It asks everything `onScroll` asks, for the same reasons, because it
   *  records the same thing from the other cause. A restore in flight is
   *  placing the reader itself, and its own write is what gets recorded.
   *
   *  And one guard of its own: it keeps a record CURRENT and never opens one.
   *  A reader who has not moved in this thread has no reading position, which
   *  is the only state the *follow seed* speaks for. Letting a streaming turn
   *  write one would take that first open away from the seed. */
  const recordContentPlace = () => {
    if (observed === undefined) return;
    if (restoring) return;
    if (isCurrent && !isCurrent()) return;
    const next = currentPosition();
    if (next === null) return;
    observed = next;
    scheduleSave();
  };

  const flush = () => {
    // A content change still inside its throttle has not been read yet, and
    // this is the last chance to read it. `recordContentPlace` asks its own
    // guards, so a superseded attachment still commits only its snapshot.
    if (contentTimer !== null) {
      clearTimeout(contentTimer);
      contentTimer = null;
      recordContentPlace();
    }
    if (saveTimer !== null) {
      clearTimeout(saveTimer);
      saveTimer = null;
    }
    // No second "is anything pending" condition: `writeNow` no-ops on an
    // unobserved key, and dedups a snapshot the timer already committed.
    writeNow();
  };

  const onScroll = () => {
    if (restoring) return;
    // A scroll arriving after this key stopped being the current one is not
    // about this key, whatever it looks like. The teardown is deferred past the
    // render that changed `key`. So the listener stays attached through a
    // window in which the shared `.thread-content` belongs to the NEXT thread.
    //
    // Two routes land in that window and both would destroy the reading
    // position in the thread being LEFT. Opening a thread with no saved
    // position resets the container to the top, and that write moves it. When
    // the thread being opened HAS one, no reset runs, but swapping in its
    // content clamps `scrollTop` instead. Either way a scroll event reaches
    // this handler carrying the incoming thread's offset.
    //
    // Asking whether this attachment is still current answers both, and asks
    // nothing about WHY the container moved. No ordering assumption about which
    // scroll listener runs first.
    if (isCurrent && !isCurrent()) return;
    // **EVERY position is recorded, including the bottom.** Nothing scrolls to
    // the bottom on its own (ADR 0064). Declining to save at the bottom would
    // therefore send a reader who finished a thread to the TOP of it on
    // re-entry. A scrollTop of 0 persists as "0", a real position distinct
    // from no save at all.
    //
    // The value is captured HERE rather than when the debounce fires: see
    // `observed`. Nothing is lost by reading it a beat earlier, every scrollTop
    // change firing this handler, so a burst's last event carries the settled
    // position.
    const next = currentPosition();
    if (next === null) return;
    observed = next;
    scheduleSave();
  };

  // A higher-priority scroll may own this load, e.g. a notification deep-link
  // resolving a scroll to a specific event. Skip the RESTORE so it cannot be
  // overridden, but still attach the save listener below, so the reader's
  // post-landing position is remembered.
  //
  // Which BRANCH this open takes is decided once, here, and that is complete
  // only for a claim already in place when the effect runs. A claim taken LATER
  // cannot be seen here, and the restore this sets up would still be armed to
  // overrule it. So the claim is DELIVERED instead of re-read (see the
  // `onDeepLinkClaimed` subscription below).
  /** Record an arm as this thread's reading position.
   *
   *  Arming a standing follow can produce NO scroll event at all. A reader
   *  already at the live edge gets a write the browser clamps to where they
   *  are, and an idle thread then grows nothing. The request is real either
   *  way, so it is recorded from the arm rather than from a scroll that may
   *  never come. Only the ARM is broadcast (see `onFollowArmed`). That is what
   *  lets `focusThread` retire the follow on a thread switch without
   *  overwriting the live edge just recorded for the thread being LEFT. */
  function subscribeToArm() {
    if (!followsLiveEdge) return null;
    return onFollowArmed(() => {
      // The same question `onScroll` asks: a superseded attachment is still
      // subscribed until its deferred teardown, and a follow armed in the
      // thread now on screen is not this key's request.
      if (isCurrent && !isCurrent()) return;
      // OUR OWN replay of what the record already says. Nothing is new, so
      // nothing below applies: the request is on disk, and the restore this
      // travels with is what puts the reader back on their turn.
      if (replayingArm) return;
      // Anything this attach still has pending is now stale: the reader has
      // asked for the live edge, which outranks any position this hook was
      // going to put them in. Both pending things would land their write ON TOP
      // of the follow and retire it in the same stroke. One is the restore
      // observers, still waiting for the transcript to grow. The other is the
      // dead-deep-link rescue, whose "has anything moved" test an arm at the
      // live edge does not trip.
      stopRestore();
      observed = LIVE_EDGE_VALUE;
      scheduleSave();
    });
  }

  // Subscribed BEFORE the positioning branch below, because that branch can
  // arm: the *follow seed* does, on a thread with no reading position. Left
  // until after it, the seeded arm broadcasts to nobody. Recording `live-edge`
  // would then depend on whether arming happened to MOVE the container, so two
  // readers doing the same thing would get different persistence.
  //
  // So a SEEDED arm records too, and that is the semantics rather than a side
  // effect. The seed decides a thread's FIRST open, and from then on the thread
  // owns the answer like any other. Turning the seed off later changes what NEW
  // threads do, not what a thread the reader has already ridden does.
  const unsubscribeArm = subscribeToArm();

  const allowRestore = openIsOurs();

  // The deep-link stand-down leads, because it is the one branch that does not
  // care what was saved: it answers for every value of `saved`, including none.
  // The rest read the saved position, and taking them in this order is what lets
  // each one narrow it.
  if (!allowRestore) {
    // A deep-link already owned this open when the effect ran, which is the
    // ordering `focusThread` produces for a thread already in the map. The
    // other orderings arrive later and reach the same place through the claim
    // broadcast below.
    standDownForDeepLink();
  } else if (saved === null) {
    // No reading position at all: a BRAND-NEW thread, or one the reader has
    // never parked in. This is the only branch the *follow seed* speaks for.
    // Every other branch is the reader's own last act on this thread, which
    // outranks a standing preference. A record therefore wins in both
    // directions.
    //
    // Gated on `followsLiveEdge` like the record's own live-edge branch, the
    // follow being one global while this hook serves three containers.
    const seeded = followsLiveEdge && applyFollowSeed(el);
    // Browsers preserve scrollTop across children-shrink, so a shared
    // container needs an explicit reset; non-shared containers opt out. Skipped
    // when the seed armed, which wrote the live edge instead and would be undone
    // by a reset to the top.
    if (!seeded && resetOnEmpty) markNavigationScroll(el, 0);
    restoring = false;
  } else if (saved.kind === 'live-edge') {
    // The reader had a standing follow armed here when they left. Resume it:
    // write today's live edge and re-arm, so everything produced while they
    // were away is behind them. No observer retry loop, unlike the offset
    // branch below. An offset needs the transcript tall enough to hold it,
    // while the live edge is wherever the content currently ends.
    resumeFollowingBottom(el);
    restoring = false;
  } else if (saved.armed) {
    // Armed, but parked away from the edge. Pick the request back up and put
    // them on their turn, in that order: the arm must be standing before the
    // restore's own write, so the recorder describes an armed reader from the
    // first scroll event. Nothing takes them to the bottom here. The growth
    // branch does, once the thread is actually running.
    replayParkedArm('parked');
    positionFromRecord();
  } else {
    positionFromRecord();
  }

  if (restoring) {
    // Still too short, so wait for it to grow. Two observers cover the two ways
    // `scrollHeight` does that after first paint:
    //   - ResizeObserver: container's own size changes (rare for flex:1
    //     containers in fixed parents, but covers initial 0→layout).
    //   - MutationObserver: subtree content changes. Children added by async
    //     Loadable<T> data leave the container's box alone, so the
    //     ResizeObserver never fires for the typical scrollable list.
    //
    // PARK the reader at the top for the wait, on a shared container. Nothing
    // else stands between them and the outgoing thread's offset while it runs.
    // `.thread-content` is one element, and the `resetOnEmpty` write is gated
    // on there being no saved position. So a saved offset that never became
    // reachable would leave the reader on a borrowed number, which the save
    // listener then persists as their own. The top is where a thread whose
    // position cannot be honoured opens, so it is the honest place to spend the
    // wait.
    if (resetOnEmpty) markNavigationScroll(el, 0);
    // Each callback is WRAPPED, never passed straight through. An observer
    // hands its entries to the first parameter and a DOM listener its event.
    // Either would arrive as `tryRestore`'s `final` or `stopRestore`'s
    // `settled`, and read as true.
    resizeObserver = new ResizeObserver(() => tryRestore());
    resizeObserver.observe(el);
    mutationObserver = new MutationObserver(() => tryRestore());
    mutationObserver.observe(el, { childList: true, subtree: true });
    deadlineTimer = setTimeout(onDeadline, RESTORE_DEADLINE_MS);
    // The wait belongs to the reader too: the first thing they DO retires it
    // (see `stopUserWatch`). Armed last, so the writes above cannot trip it.
    stopUserWatch = watchUserAction(() => stopRestore());
  }

  /** Arm the content watch, for an ANCHORING container only.
   *
   *  A container recording a bare offset needs none of this. Its answer IS
   *  `scrollTop`, which cannot change without a scroll event. Only a named
   *  child can start or stop being nameable under a still reader.
   *
   *  Both arms are load-bearing. A page of older turns lands as a child-list
   *  change. A fragment folding into its real turn only stamps the attribute,
   *  on a node Preact keeps, since `exchangeKey` does not change with the fold. */
  const watchContent = () => {
    if (!anchorsToContent || typeof MutationObserver !== 'function') return;
    contentObserver = new MutationObserver(() => {
      if (contentTimer !== null) return;
      contentTimer = setTimeout(() => {
        contentTimer = null;
        recordContentPlace();
      }, CONTENT_READ_THROTTLE_MS);
    });
    contentObserver.observe(el, { childList: true, subtree: true, attributeFilter: [ANCHOR_ATTR] });
  };
  watchContent();

  el.addEventListener('scroll', onScroll, { passive: true });


  // A deep-link CLAIMING the open retires a restore still armed, for the same
  // reason the arm above does. `allowRestore` answers only for a claim already
  // in place, and the two orderings it cannot see are ordinary. A deep-link
  // into the thread the reader is ALREADY in re-attaches nothing. A thread
  // whose events arrive while the tap resolves attaches first, with no claim to
  // see. Either way the claim renders the FULL exchange list, and that growth
  // is what the waiting restore has been waiting for, so the two collide.
  // `openIsOurs` scopes this to the container the claim is about.
  //
  // It takes the SAME stand-down the attach-time branch takes, rescue included,
  // rather than merely retiring the restore. Standing down is two obligations.
  //
  // Two states answer this claim and nothing else does. A RESTORE is armed,
  // which the stand-down retires. Or a RESCUE is in flight from an earlier
  // claim, whose budget this newer claim extends. A claim arriving after the
  // deadline into a transcript that never grew tall enough gets no rescue. It
  // needs no `isCurrent` guard, unlike the arm above: its rescue asks
  // `isCurrent` before it writes, as does `recordDeepLinkLanding`.
  const unsubscribeDeepLink = onDeepLinkClaimed(() => {
    if (!restoring && deadlineTimer === null) return;
    if (openIsOurs()) return;
    standDownForDeepLink();
  });

  // The link FOUND its target, which settles both halves of what this
  // attachment owes it.
  //
  // It is positioning the reader, so the rescue has nothing left to cover. Told
  // rather than inferred. The rescue's own "has anything moved" test reads a
  // landing with nowhere to move as a dead link. It would then haul the reader
  // off the event they are looking at. Only ever cancels a rescue in flight,
  // `restoring` true meaning the restore deadline holds that slot.
  //
  // And where it landed is this thread's reading position, so it is RECORDED.
  //
  // `openIsOurs` scopes both to the container the link is about, which is why
  // the announcement is made while the claim is still held. The other guard
  // lives inside `recordDeepLinkLanding` rather than here, so it covers the
  // other call site too. It catches a superseded attachment hearing a landing
  // that belongs to the thread now on screen.
  const unsubscribeDeepLinkResolved = onDeepLinkResolved(() => {
    if (openIsOurs()) return;
    if (!restoring && deadlineTimer !== null) stopRestore();
    recordDeepLinkLanding();
    // The link placed the reader, so nothing else will. This is the other end
    // of the stand-down's hand-over, reached when the link lives.
    settleRestore();
  });

  // Backgrounding the app is not a teardown, so nothing here would otherwise
  // commit. The save is debounced and flushed from the cleanup, and a frozen
  // page's pending timer never runs if the page is discarded. The damaging
  // direction is a lost DISARM. The reader scrolls up and backgrounds, and the
  // stale live-edge record outlives them. The next open then drags them to the
  // bottom having asked for the opposite. Every other debounced writer in the
  // app already flushes here.
  //
  // `flush` commits the SNAPSHOT taken while this key was current, which is why
  // this one asks no `isCurrent()` question. There is nothing here for a later
  // render to have moved. It tears nothing down either, so the same attachment
  // keeps recording after the paired wake.
  const unsubscribeHide = onPageHide(flush);

  // Coming back is a re-entry: the reader has been away and returned, so the
  // thread is positioned by the same rule that positions it when they arrive
  // from another thread. Only the transcript, like the arm subscription.
  //
  // **It reads the RECORD rather than the follow flag.** The flag survives a
  // suspend and dies on a discard, and the wake ITSELF can destroy it. A
  // bfcache scroll restore fires an event shaped like the reader taking the
  // container away from the follow, which is the disarm. The same warning is
  // in `utils/pageResume.ts`. Reading the record makes the answer independent
  // of when the browser dispatches that restore, and gives the suspend and
  // discard paths one answer. The flush above makes the record trustworthy.
  const unsubscribeWake = followsLiveEdge
    ? onPageWake(() => {
        if (isCurrent && !isCurrent()) return;
        // A notification tap can resume the app and resolve a deep-link in one
        // breath: the app comes back, the link lands on the event, and this
        // fires. The deep-link owns the viewport, same as at attach time, so
        // the resume writes nothing there and only picks the request back up.
        //
        // A reader who was PARKED when they backgrounded is written nowhere
        // either way. Their ride carries them only once the thread runs, and
        // waking is not the thread running.
        replayRecordedFollow(readSaved(), openIsOurs() ? 'live-edge' : 'in-place');
      })
    : null;

  return () => {
    // NOT settled. The same key re-attaches whenever `paused` flips, which a
    // corrupt-events rebuild does, and the fresh attachment needs whatever the
    // restore had running on its behalf. A teardown for a real thread switch
    // says nothing either way, `settleRestore` being scoped to the current key.
    stopRestore(false);
    // Before `flush`, which reads any throttled content change out for itself.
    // Leaving this connected would keep measuring a container the next thread
    // owns, under a key nothing will write again.
    contentObserver?.disconnect();
    contentObserver = null;
    el.removeEventListener('scroll', onScroll);
    unsubscribeArm?.();
    unsubscribeDeepLink();
    unsubscribeDeepLinkResolved();
    unsubscribeHide();
    unsubscribeWake?.();
    flush();
  };
}

/** Persist and restore a scroll container's vertical scroll position across
 *  reloads via localStorage. The container is identified by `key`; the same key
 *  on next mount restores the saved offset.
 *
 *  When `key` changes (e.g., switching threads), the previous container's
 *  position is flushed and the new key is restored.
 *
 *  A thin `useEffect` over `attachScrollMemory`, which owns the behaviour. The
 *  live options are handed over as a getter closing over the latest render's
 *  values. So the attachment sees the current ones without the effect having to
 *  re-run, which on every new turn would tear down the restore observers
 *  mid-load. */
export function useScrollMemory(
  ref: RefObject<HTMLElement>,
  key: string | null,
  options: ScrollMemoryOptions = {},
) {
  const {
    paused = false,
    resetOnEmpty = false,
    followsLiveEdge = false,
    anchorsToContent = false,
  } = options;
  const liveRef = useRef<ScrollMemoryLive>(options);
  liveRef.current = options;
  // What the LATEST render says the key is, versus the one an attachment was
  // set up for. They differ for exactly as long as a superseded attachment is
  // still listening, which is the window `onScroll`'s first guard closes.
  const keyRef = useRef(key);
  keyRef.current = key;

  useEffect(() => {
    if (!key || paused) return;
    const el = ref.current;
    if (!el) return;
    return attachScrollMemory(el, key, {
      live: () => liveRef.current,
      resetOnEmpty,
      followsLiveEdge,
      anchorsToContent,
      isCurrent: () => keyRef.current === key,
    });
  }, [ref, key, paused, resetOnEmpty, followsLiveEdge, anchorsToContent]);
}
