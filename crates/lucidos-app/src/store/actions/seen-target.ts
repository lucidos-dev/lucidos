/** The *seen target* rule: a notification clears once the reader has actually
 *  looked at what it points at, however they got there. A *read request*
 *  clears the same way, once the end of the reply it points at has been on
 *  screen (ADR 0409).
 *
 *  Row 1 of the §4 matrix already does this, but only once, on the SSE that
 *  announces the notification. Reaching the same event later left the row
 *  unread. That covers the Blocked list, and foregrounding an app that
 *  had the thread open. This module makes the same test standing.
 *
 *  The test is the strict one. For a tap naming an event, the reader has seen
 *  it when that event's card is in the transcript's visible band. The measure
 *  is the `isInViewport` the pong already uses. A tap naming a place with no
 *  card is seen when that place is on screen. Either way it must hold for
 *  `SEEN_DWELL_MS`.
 *
 *  Because the test IS Row 1's test, the arrival matrix is undisturbed. Row 2
 *  keeps its toast, since a card scrolled out of band fails here too. See
 *  `system-knowhow/notifications.md` §4 and
 *  `docs/plans/2026-08-30-visiting-a-tap-target-marks-its-notifications-read.md`.
 */

import { effect, untracked } from '@preact/signals';
import {
  activeMenuItem,
  focusedThreadId,
  mobileView,
  panelOverlay,
  settingsSubview,
  splitRatio,
  threadMap,
  threadUnsettled,
  unreadNotifications,
} from '../store';
import type { MobileView, PanelOverlay, SettingsSubview } from '../store';
import type { MenuItem } from '../types';
import { isInViewport, isReplyEndInViewport, lastExchangeOf, viewportIsMobile } from '../../utils/viewport';
import { markReplySeen } from '../../api/threads';
import { postClientLog } from '../../utils/clientLog';
import { isPageActive } from '../../utils/pageActive';
import { onPageHide, onPageWake } from '../../utils/pageVisit';
import { markReadOptimistic } from './notifications';
import {
  appVisitKey,
  fileVisitKey,
  notificationTarget,
  panelVisitKey,
  settingsVisitKey,
  threadVisitKey,
  triggerVisitKey,
} from './visitKeys';
import type { SeenTarget } from './visitKeys';

/** How long the target must hold before the notification clears.
 *
 *  A glimpse is not a read. Four focus changes happen without the reader
 *  asking. Drawer browsing with the arrow keys moves the focused thread per
 *  keypress. A mobile swipe passes through the middle pane. Archiving hands
 *  focus to the next row. A deep-link bootstrap focuses optimistically before
 *  its fetch lands.
 *
 *  A fast scroll flings cards through the band the same way. One dwell covers
 *  all five, because each stops being true again within it. */
export const SEEN_DWELL_MS = 1000;

// ---------------------------------------------------------------------------
// Where the reader is (pure)
// ---------------------------------------------------------------------------

/** Everything the seen test needs about the shell's layout. A record rather
 *  than signal reads, so the rules below are testable with no store and no
 *  DOM. */
export interface VisitLocation {
  focusedThreadId: string | null;
  overlay: PanelOverlay;
  activeMenuItem: MenuItem;
  settingsSubview: SettingsSubview;
  mobile: boolean;
  mobileView: MobileView;
  splitRatio: number;
}

/** Is the Conversation pane on screen? Mobile navigates between panes, so only
 *  the current one is. Desktop shows both unless the split collapsed one. */
export function threadPaneOnScreen(loc: VisitLocation): boolean {
  return loc.mobile ? loc.mobileView === 'thread' : loc.splitRatio > 0;
}

/** Is the Canvas pane on screen? The mirror of `threadPaneOnScreen`. */
export function contentPaneOnScreen(loc: VisitLocation): boolean {
  return loc.mobile ? loc.mobileView === 'content' : loc.splitRatio < 1;
}

/** The place the Canvas pane shows, or null when it shows nothing a
 *  notification can point at.
 *
 *  Close to `contentViewKey`, but not the same question, so not shared. That
 *  one asks whether the pane navigated, and deliberately gives every app ONE
 *  key, because an app switch reuses the iframe. Here the app's identity IS the
 *  answer. A url preview and a notification detail are null: no tap names
 *  them. */
function contentVisitKey(loc: VisitLocation): string | null {
  const overlay = loc.overlay;
  if (overlay) {
    if (overlay.type === 'app-ui') return appVisitKey(overlay.app.id);
    if (overlay.type === 'file-preview') return fileVisitKey(overlay.path);
    if (overlay.type === 'form' && overlay.form.type === 'trigger' && overlay.form.triggerId) {
      return triggerVisitKey(overlay.form.triggerId);
    }
    return null;
  }
  if (loc.activeMenuItem === 'settings') return settingsVisitKey(loc.settingsSubview);
  return panelVisitKey(loc.activeMenuItem);
}

/** The card-less places on screen right now, at most one per pane. A desktop
 *  split really does put both in front of the reader, so both count. */
export function visitedKeys(loc: VisitLocation): string[] {
  const keys: string[] = [];
  if (loc.focusedThreadId && threadPaneOnScreen(loc)) {
    keys.push(threadVisitKey(loc.focusedThreadId));
  }
  if (contentPaneOnScreen(loc)) {
    const key = contentVisitKey(loc);
    if (key) keys.push(key);
  }
  return keys;
}

// ---------------------------------------------------------------------------
// The dwell (pure)
// ---------------------------------------------------------------------------

/** Dwell bookkeeping. `since` holds when each satisfied notification became so.
 *  `done` holds the ones already reported, so a failed read is not re-reported
 *  on every sample. */
export interface SeenWaits {
  since: Map<string, number>;
  done: Set<string>;
}

export function emptySeenWaits(): SeenWaits {
  return { since: new Map(), done: new Set() };
}

/** Advance the dwell over one sample and return what just completed it.
 *  Mutates `waits`.
 *
 *  A target that dropped out of `satisfied` loses its wait entirely, so coming
 *  back starts it from zero. That is the whole anti-glimpse rule: time in the
 *  band has to be continuous, never accumulated across passes. */
export function sampleSeen(
  waits: SeenWaits,
  now: number,
  satisfied: readonly string[],
  dwellMs: number = SEEN_DWELL_MS,
): string[] {
  const live = new Set(satisfied);
  for (const id of [...waits.since.keys()]) {
    if (!live.has(id)) waits.since.delete(id);
  }
  const completed: string[] = [];
  for (const id of live) {
    if (waits.done.has(id)) continue;
    const started = waits.since.get(id);
    if (started === undefined) {
      waits.since.set(id, now);
      continue;
    }
    if (now - started >= dwellMs) {
      waits.since.delete(id);
      waits.done.add(id);
      completed.push(id);
    }
  }
  return completed;
}

/** Forget every notification the page no longer holds as unread, so neither
 *  collection grows across a session. */
export function pruneSeenWaits(waits: SeenWaits, keep: ReadonlySet<string>): void {
  for (const id of [...waits.since.keys()]) if (!keep.has(id)) waits.since.delete(id);
  for (const id of [...waits.done]) if (!keep.has(id)) waits.done.delete(id);
}

/** How long until the next dwell completes, or null when nothing is waiting. */
export function nextDwellDueIn(
  waits: SeenWaits,
  now: number,
  dwellMs: number = SEEN_DWELL_MS,
): number | null {
  let earliest: number | null = null;
  for (const started of waits.since.values()) {
    if (earliest === null || started < earliest) earliest = started;
  }
  if (earliest === null) return null;
  return Math.max(0, earliest + dwellMs - now);
}

// ---------------------------------------------------------------------------
// The watch (impure shell)
// ---------------------------------------------------------------------------

/** The end of a thread's newest reply, which a read request points at. */
interface ReplyEndTarget {
  kind: 'reply-end';
  threadId: string;
}

/** One thing the rule waits for the reader to see, and what seeing it does. */
interface WatchedTarget {
  id: string;
  target: SeenTarget | ReplyEndTarget;
  onSeen: () => void;
}

const waits = emptySeenWaits();
let watched: WatchedTarget[] = [];
let observer: IntersectionObserver | null = null;
let observed: Element[] = [];
let renderWatch: MutationObserver | null = null;
let renderWatchTimer: ReturnType<typeof setTimeout> | null = null;
let dwellTimer: ReturnType<typeof setTimeout> | null = null;
/** Cancels the frame or timeout a pending sample is riding, whichever it took.
 *  Holding the canceller rather than a bare id is what lets both branches of
 *  `scheduleResample` be torn down through one call. */
let cancelCoalesced: (() => void) | null = null;
let installed = false;

function currentLocation(): VisitLocation {
  return {
    focusedThreadId: focusedThreadId.value,
    overlay: panelOverlay.value,
    activeMenuItem: activeMenuItem.value,
    settingsSubview: settingsSubview.value,
    mobile: viewportIsMobile.value,
    mobileView: mobileView.value,
    splitRatio: splitRatio.value,
  };
}

/** Which watched notifications are at their target right now.
 *
 *  An event target needs three things: its thread focused, that pane on screen,
 *  and its card in the band. The first two stop a card in a mounted but
 *  off-screen mobile pane from counting. `isInViewport` rejects that case too,
 *  so asking here is for readability. */
function satisfiedIds(): string[] {
  const loc = currentLocation();
  const places = new Set(visitedKeys(loc));
  const onThreadPane = threadPaneOnScreen(loc);
  const ids: string[] = [];
  for (const { id, target } of watched) {
    if (target.kind === 'place') {
      if (places.has(target.key)) ids.push(id);
      continue;
    }
    if (!onThreadPane || loc.focusedThreadId !== target.threadId) continue;
    const inBand = target.kind === 'event'
      ? isInViewport(target.eventId)
      : isReplyEndInViewport(target.threadId);
    if (inBand) ids.push(id);
  }
  return ids;
}

/** The watch id of `threadId`'s read request. Prefixed so it can never
 *  collide with a notification id in the shared waits. */
function readRequestWatchId(threadId: string): string {
  return `read-request:${threadId}`;
}

/** Re-read the unread set and the read requests, and rebuild what we watch.
 *  The only thing that grows or shrinks `watched`. */
function rearm(): void {
  watched = [];
  const keep = new Set<string>();
  const set = unreadNotifications.value;
  // Not knowing the unread set is not the same as knowing it is empty. Watch
  // none of it, keep its waits, and let the load that lands re-arm us. That is
  // the cold-open case: the thread is already on screen while the startup
  // fetch is still in flight.
  const notificationsKnown = set.status === 'loaded';
  if (notificationsKnown) {
    for (const notification of set.data) {
      keep.add(notification.id);
      const target = notificationTarget(notification.tap);
      if (target) watched.push({ id: notification.id, target, onSeen: () => markReadOptimistic(notification.id) });
    }
  }
  // A request counts only once the turn has ended, as Review lists it.
  for (const thread of threadMap.value.values()) {
    if (thread.meta.readRequested !== true || threadUnsettled(thread)) continue;
    const threadId = thread.meta.id;
    const id = readRequestWatchId(threadId);
    keep.add(id);
    watched.push({ id, target: { kind: 'reply-end', threadId }, onSeen: () => reportReplySeen(threadId) });
  }
  if (notificationsKnown) pruneSeenWaits(waits, keep);
}

/** Tell the engine the user saw the reply, as of the summary this page holds.
 *  Its `ThreadReplySeen` clears the request on every device. A failure, or a
 *  409 for a summary that moved on, drops the dwell's `done` mark so the next
 *  sample can report again. */
function reportReplySeen(threadId: string): void {
  const version = threadMap.value.get(threadId)?.meta.summaryVersion;
  if (version === undefined) return;
  markReplySeen(threadId, version).catch((e: unknown) => {
    postClientLog('seen-target', 'reporting a read reply failed', { threadId, error: String(e) });
    waits.done.delete(readRequestWatchId(threadId));
    // A reader holding still fires no scroll or signal, so ask for the next
    // sample here, or the retry the dropped mark allows never comes.
    scheduleResample();
  });
}

function clearDwellTimer(): void {
  if (dwellTimer !== null) clearTimeout(dwellTimer);
  dwellTimer = null;
}

/** How long a render watch waits for its card before giving up.
 *
 *  It has to expire, because "the card has not rendered" is a permanent state
 *  for a real and ordinary case: the transcript is WINDOWED, so an event older
 *  than the rendered slice is genuinely absent while the reader sits in that
 *  thread. An unbounded watch would then run a whole-body `MutationObserver`
 *  for the rest of that visit, sampling on every streamed token.
 *
 *  Expiring costs nothing the rule needs. Every other trigger re-arms it with a
 *  fresh deadline. A card that renders later is still caught the next time the
 *  reader scrolls or navigates, or the thread grows. */
export const RENDER_WATCH_MS = 5000;

function clearRenderWatch(): void {
  renderWatch?.disconnect();
  renderWatch = null;
  if (renderWatchTimer !== null) clearTimeout(renderWatchTimer);
  renderWatchTimer = null;
}

function disarmViewportObserver(): void {
  observer?.disconnect();
  observer = null;
  observed = [];
  clearRenderWatch();
}

/** Wait for a card that has not rendered yet.
 *
 *  Arming the observer needs the element, and the sample that arms it can run
 *  before Preact has committed the transcript. Nothing else fires afterwards, so
 *  the rule slept with its card plainly on screen. That is a race, and it is why
 *  this was a desktop flake: on mobile the test's own pane swipe happened to
 *  re-sample after the commit.
 *
 *  Bounded twice over. It arms only for a card whose thread is ALREADY focused,
 *  and it runs for at most `RENDER_WATCH_MS`. It disconnects the moment the card
 *  appears. */
function armRenderWatch(missing: boolean): void {
  if (!missing) {
    clearRenderWatch();
    return;
  }
  if (renderWatch) return;
  if (typeof MutationObserver !== 'function' || typeof document === 'undefined') return;
  if (!document.body) return;
  renderWatch = new MutationObserver(scheduleResample);
  renderWatch.observe(document.body, { childList: true, subtree: true });
  renderWatchTimer = setTimeout(clearRenderWatch, RENDER_WATCH_MS);
}

function sameElements(a: readonly Element[], b: readonly Element[]): boolean {
  return a.length === b.length && a.every((el, i) => el === b[i]);
}

/** Watch the cards this rule is waiting on. A card can enter the band with no
 *  scroll and no navigation, and this is what samples it there.
 *
 *  It is the CHANGE NOTIFIER only, never the verdict: the band is
 *  `isInViewport`'s to define, and the observer runs on the plain viewport. The
 *  two agree for the purpose, because scrolling an inner container moves the
 *  card in viewport coordinates too.
 *
 *  This closes the gap the other subscriptions cannot. A transcript reaches its
 *  final layout AFTER the render that produced it, through a restored reading
 *  position and the reflows around it. Nothing else fires there, so a card out
 *  of band at render time waited for a scroll that never came. Observing it
 *  delivers an initial entry, which is the settle sample.
 *
 *  Re-arming only on a real change is what stops that initial entry becoming a
 *  loop: an unchanged set returns before touching the observer. */
function armViewportObserver(): void {
  if (typeof IntersectionObserver !== 'function' || typeof document === 'undefined') return;
  const els: Element[] = [];
  let missing = false;
  for (const { target } of watched) {
    if (target.kind === 'place') continue;
    if (focusedThreadId.value !== target.threadId) continue;
    if (target.kind === 'reply-end') {
      // The scroll listener samples the edge as it moves. The observer only
      // has to notice the turn arriving in the band at all.
      const last = lastExchangeOf(target.threadId);
      if (last) els.push(last);
      else missing = true;
      continue;
    }
    const found = document.querySelectorAll(`[data-event-id="${CSS.escape(target.eventId)}"]`);
    if (found.length === 0) missing = true;
    for (const el of found) els.push(el);
  }
  if (!sameElements(els, observed)) {
    observer?.disconnect();
    observer = null;
    observed = els;
    if (els.length > 0) {
      observer = new IntersectionObserver(scheduleResample);
      for (const el of els) observer.observe(el);
    }
  }
  // After the re-arm, never before: the re-arm tears the previous observers
  // down, and a render watch armed first would go down with them.
  armRenderWatch(missing);
}

/** Take one sample and mark read whatever completed its dwell. */
export function resampleSeenTargets(): void {
  clearDwellTimer();
  if (!isPageActive()) {
    // Not looking, so nothing is being seen. Drop the waits rather than pause
    // them. A dwell that survived a background would let a card the reader
    // never came back to complete on time spent away.
    waits.since.clear();
    return;
  }
  if (watched.length === 0) {
    disarmViewportObserver();
    return;
  }
  armViewportObserver();
  const now = Date.now();
  const completed = new Set(sampleSeen(waits, now, satisfiedIds()));
  for (const { id, onSeen } of watched) if (completed.has(id)) onSeen();
  const due = nextDwellDueIn(waits, now);
  if (due !== null) dwellTimer = setTimeout(resampleSeenTargets, due);
}

function clearCoalesced(): void {
  if (cancelCoalesced) cancelCoalesced();
  cancelCoalesced = null;
}

/** Stop watching and drop every wait in flight. The reader is not looking, so
 *  no dwell may keep accruing. */
function standDown(): void {
  clearCoalesced();
  clearDwellTimer();
  waits.since.clear();
}

/** Coalesce the several signals that move together into one sample per frame.
 *  A scroll fires far faster than the rules can change. */
function scheduleResample(): void {
  if (cancelCoalesced) return;
  const run = () => {
    cancelCoalesced = null;
    resampleSeenTargets();
  };
  if (typeof requestAnimationFrame === 'function') {
    const frame = requestAnimationFrame(run);
    cancelCoalesced = () => cancelAnimationFrame(frame);
  } else {
    const timer = setTimeout(run, 0);
    cancelCoalesced = () => clearTimeout(timer);
  }
}

/** Subscribe to everything that can change the answer, and nothing else.
 *
 *  No polling. Five subscriptions here, plus the per-card observer each sample
 *  arms. The scroll listener is capture-phase on `document` because scroll does
 *  not bubble, and the transcript's scroll container is remounted by every
 *  layout switch. One listener beats resolving and re-binding that element.
 *
 *  It subscribes to the reader's own viewport, not to server state. The rule
 *  against visibility and focus listeners in `.claude/rules/frontend.md` is
 *  about the latter. */
export function installSeenTargetWatch(): void {
  if (installed) return;
  installed = true;

  effect(() => {
    // Reading the location IS the subscription to every signal in it.
    currentLocation();
    untracked(scheduleResample);
  });
  effect(() => {
    unreadNotifications.value;
    untracked(() => {
      rearm();
      scheduleResample();
    });
  });
  effect(() => {
    // A transcript that renders or streams can bring a watched card into the
    // band with no scroll and no navigation at all. A thread's read request
    // arriving or clearing changes what is watched.
    threadMap.value;
    untracked(() => {
      rearm();
      scheduleResample();
    });
  });

  if (typeof document !== 'undefined' && typeof document.addEventListener === 'function') {
    document.addEventListener('scroll', scheduleResample, { capture: true, passive: true });
  }
  if (typeof window !== 'undefined' && typeof window.addEventListener === 'function') {
    window.addEventListener('resize', scheduleResample, { passive: true });
    // `blur` and `focus`, which `utils/pageVisit.ts` deliberately does NOT
    // treat as away and back. That is right for its consumers and wrong here.
    // On desktop `isPageActive()` reads `document.hasFocus()`, so a window
    // losing OS focus while staying visible stops being active with no hide
    // anywhere. Without these two, a reader who comes back to a visible card
    // gets nothing, and the stale wait completes on time spent in another app.
    //
    // Standing down directly rather than through the coalescer. A blurred
    // window's frames are throttled, so the pending sample could land after the
    // reader returned. It would read as active and keep the wait it should
    // drop.
    window.addEventListener('blur', standDown);
    window.addEventListener('focus', scheduleResample);
  }
  onPageHide(standDown);
  onPageWake(scheduleResample);
}

/** Test-only: drop every watch and wait so one suite cannot leak into the next.
 *  The installed listeners are module-level singletons and stay. */
export function _resetSeenTargetWatchForTesting(): void {
  clearCoalesced();
  clearDwellTimer();
  disarmViewportObserver();
  waits.since.clear();
  waits.done.clear();
  watched = [];
}
