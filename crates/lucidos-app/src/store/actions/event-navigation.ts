import { effect, signal } from '@preact/signals';
import { ApiError } from '../../api/client';
import { fetchEventLocation } from '../../api/threads';
import { EVENT_RESOLVE_DEADLINE_MS } from '../../components/chat/scrollState';
import { computeExchanges, deepLinkAnchorForEvent } from '../thread-events';
import { showToast, threadMap, focusedThreadId } from '../store';
import { errorDetail } from '../../utils/errorDetail';
import { ensureThreadByIdInMap, loadThreadEvents } from './thread-loading';
import { focusThread } from './threads';

/** A workspace domain event belongs to no conversation, so there is no
 *  transcript to open. A real answer, not a failure. */
const NOT_IN_A_CONVERSATION =
  'That event is a workspace event, not part of any conversation, so there is nowhere to open it.';

/** The event is in a thread, but nothing in that thread's transcript draws it
 *  or was caused by it, so a jump has no honest destination. See
 *  `deepLinkAnchorForEvent`. */
const NOTHING_TO_LAND_ON =
  'That event is not drawn anywhere in its thread, so there is nothing to jump to.';

/** Where a jump to one event would land, as far as can be known WITHOUT
 *  fetching another thread's events.
 *
 *  That boundary is the whole point of the type. Resolving an event fully means
 *  loading its thread's entire history, which is far too expensive to do while
 *  merely RENDERING a row that might offer a jump; the cheap half is enough to
 *  decide whether the affordance should exist at all, and the expensive half
 *  stays on the click.
 *
 *  `nowhere` carries the words a toast would use, so the two callers cannot
 *  drift on what "no target" means. */
export type EventTarget =
  /** Nothing to open. The row shows no jump; a click that races the answer
   *  toasts `note`. */
  | { kind: 'nowhere'; note: string }
  /** The owning thread's events are already in the store, so the exact element
   *  to pulse is known right now. */
  | { kind: 'anchored'; threadId: string; anchor: string }
  /** Owned by a thread whose events are not loaded. There is somewhere to go;
   *  which element is a question only that thread's events can answer. */
  | { kind: 'unloaded'; threadId: string };

/** Resolved `eventId -> thread_id` answers. Module-level and unbounded on
 *  purpose: an event's owning thread never changes, the value is one nullable
 *  uuid, and the alternative is re-asking the engine every time a transcript
 *  re-renders. */
const owningThreads = new Map<string, string | null>();

/** In-flight location requests, so N rows asking about the same event id make
 *  ONE request. A wake card and the trigger row for the same fire are the
 *  everyday case; a re-render storm is the pathological one. */
const owningThreadRequests = new Map<string, Promise<string | null>>();

/** The thread holding `eventId`, or `null` when it belongs to none.
 *
 *  Checks the focused thread's already-loaded events before asking the engine.
 *  A wait can match an event in its OWN thread, and on an iOS PWA over Tailscale
 *  a round-trip is 400 to 800 ms steady state, which is long enough for a tap to
 *  read as dead. Only the focused thread is scanned: it is the one the user is
 *  looking at, and sweeping every loaded thread's event map would trade a
 *  bounded fetch for an unbounded walk.
 *
 *  A failed request caches nothing and clears its in-flight entry, so a dropped
 *  connection costs one retry rather than a permanently wrong answer. */
async function owningThreadId(eventId: string): Promise<string | null> {
  const openId = focusedThreadId.value;
  const open = openId ? threadMap.value.get(openId) : null;
  if (open) {
    for (const event of open.events.values()) {
      if (event._eventId === eventId) return openId;
    }
  }
  const cached = owningThreads.get(eventId);
  if (cached !== undefined) return cached;
  const inFlight = owningThreadRequests.get(eventId);
  if (inFlight) return inFlight;
  const request = fetchEventLocation(eventId)
    .then(location => {
      owningThreads.set(eventId, location.thread_id);
      return location.thread_id;
    })
    .finally(() => {
      owningThreadRequests.delete(eventId);
    });
  owningThreadRequests.set(eventId, request);
  return request;
}

/** The anchor for `eventId` within a thread whose events are in the store, or
 *  `null` when that thread draws nothing the jump can honestly land on. */
function anchorInLoadedThread(threadId: string, eventId: string): string | null {
  const thread = threadMap.value.get(threadId);
  // `computeExchanges` is memoized per thread (`groupIntoExchangesCached`), so
  // this is a map lookup on the second and later call rather than a re-grouping
  // of the whole history.
  return thread ? deepLinkAnchorForEvent(computeExchanges(thread), eventId) : null;
}

/** Does this thread's loaded history contain the event at all?
 *
 *  The question a null anchor cannot answer on its own, and the two answers want
 *  opposite handling. **The thread has it and draws nothing for it** is final:
 *  there is no element and there never will be. **The thread does not have it
 *  yet** is a race, and a real one: `GET /events/:id/location` reads the event
 *  store directly, so it knows about an event that a thread snapshot taken a
 *  moment earlier missed and SSE has not delivered. Treating that as final would
 *  refuse a jump that is about to work. */
function loadedThreadHolds(threadId: string, eventId: string): boolean {
  const thread = threadMap.value.get(threadId);
  if (!thread?.eventsLoaded) return false;
  for (const event of thread.events.values()) {
    if (event._eventId === eventId) return true;
  }
  return false;
}

/** Decide what a jump to `eventId` would find, cheaply.
 *
 *  Two tiers, and the split is what keeps this callable at render time:
 *
 *   - **Which thread**, from `GET /events/:id/location` (cached, deduped), or
 *     for free when the event is in the thread already on screen. A `null`
 *     thread id is a real answer: a workspace domain event belongs to no
 *     conversation.
 *   - **Which element**, but ONLY when the owning thread's events are already
 *     here. `deepLinkAnchorForEvent` needs a thread's whole history, and fetching
 *     one to decide whether to draw a link would spend a thread load per row.
 *     An owning thread that is not loaded therefore answers `unloaded`: there IS
 *     somewhere to go, and the click works out where.
 *
 *  Never throws for the two definite "no" answers; a transport failure still
 *  rejects, so the click can report it and the render path can decline to cache
 *  it. */
export async function resolveEventTarget(eventId: string): Promise<EventTarget> {
  const threadId = await owningThreadId(eventId);
  if (!threadId) return { kind: 'nowhere', note: NOT_IN_A_CONVERSATION };
  // Only a thread that is loaded AND already holds the event can answer. A
  // thread row with no events yet makes every anchor trivially null, and a
  // loaded thread that has not caught up with the event is the same trap one
  // race later, so `nowhere` below means "drawn nowhere", never "not here yet".
  if (!loadedThreadHolds(threadId, eventId)) return { kind: 'unloaded', threadId };
  const anchor = anchorInLoadedThread(threadId, eventId);
  return anchor
    ? { kind: 'anchored', threadId, anchor }
    : { kind: 'nowhere', note: NOTHING_TO_LAND_ON };
}

/** Settled answers to "does this event have somewhere to go". */
const eventTargetVerdicts = signal<ReadonlyMap<string, boolean>>(new Map());

/** Whether a jump to `eventId` has somewhere to go, as far as is known SO FAR.
 *
 *  False while the answer is still being resolved, which is the direction that
 *  matters: a row that starts plain and gains a link reads as the answer
 *  arriving, where a link that appears and then vanishes reads as a bug and can
 *  be tapped in between.
 *
 *  Reads one small signal, deliberately not `threadMap`: this is called from
 *  inside the transcript, and subscribing a row there to the thread map would
 *  re-render it on every event of every thread. The verdict is settled once per
 *  event id by `ensureEventTargetResolved` instead, and revised by
 *  `reportNoTarget` when a click discovers the cheap answer was optimistic. */
export function eventHasTarget(eventId: string | undefined): boolean {
  return !!eventId && eventTargetVerdicts.value.get(eventId) === true;
}

/** Event ids whose verdict is being worked out, so N rows asking at once
 *  resolve once. Separate from the location dedupe above, which only covers the
 *  request: two rows can ask before either has reached the fetch. */
const verdictsInFlight = new Set<string>();

function recordVerdict(eventId: string, hasTarget: boolean): void {
  const next = new Map(eventTargetVerdicts.value);
  next.set(eventId, hasTarget);
  eventTargetVerdicts.value = next;
}

/** Settle `eventHasTarget` for this event, once.
 *
 *  Called from an effect rather than during render, both because it starts a
 *  request and because `resolveEventTarget` reads the store: doing it in a
 *  render body would subscribe the row to everything it touches. */
export function ensureEventTargetResolved(eventId: string | undefined): void {
  if (!eventId) return;
  if (eventTargetVerdicts.value.has(eventId) || verdictsInFlight.has(eventId)) return;
  verdictsInFlight.add(eventId);
  void resolveEventTarget(eventId)
    .then(target => recordVerdict(eventId, target.kind !== 'nowhere'))
    .catch(e => {
      // A 404 is a definite answer (the event has left the event store), so it
      // settles as "no target" like any other. Anything else is a transport
      // failure, and caching one dropped request as a permanent verdict would
      // silently remove a working affordance, so it is left unresolved for the
      // next mount to retry.
      //
      // Deliberately no toast, under `.claude/rules/frontend.md`'s best-effort
      // carve-out. A toast is wrong because nothing was clicked: this runs while
      // a transcript PAINTS, so a flaky connection would fire one per wake card
      // on screen for a question the user never asked. The user still loses
      // nothing they can act on: the row states the event's name either way, and
      // all that is missing is a shortcut. And it recovers on its own, since the
      // next render of the row asks again.
      if (e instanceof ApiError && e.httpCode === 404) recordVerdict(eventId, false);
    })
    .finally(() => verdictsInFlight.delete(eventId));
}

/** Forget every cached location and verdict.
 *
 *  Test-only, and there is deliberately no production caller: an event's owning
 *  thread is fixed for the life of the event, so nothing in the app can make one
 *  of these answers wrong. Only a test, which reuses ids across cases, needs the
 *  slate cleared. */
export function _resetEventTargetCacheForTesting(): void {
  owningThreads.clear();
  owningThreadRequests.clear();
  verdictsInFlight.clear();
  eventTargetVerdicts.value = new Map();
}

/** Report a dead end, and RETIRE the affordance that led to it.
 *
 *  The second half is the point. `resolveEventTarget` answers `unloaded` for any
 *  event in a thread it has not loaded, which the row reads as "there is
 *  somewhere to go" and draws a link for. That is the right optimism at render
 *  time (the alternative is a thread load per row), but it can be WRONG, and the
 *  click is where it is found out: load the thread, compute the anchor, discover
 *  there is none. Toasting and leaving the verdict at `true` would leave the chip
 *  a link that can never navigate, toasting again on every tap for the rest of
 *  the session.
 *
 *  So a definitive no revises the verdict, and the chip goes back to being plain
 *  text. Only definitive answers come through here: a transport failure reports
 *  itself in the caller's catch and settles nothing. */
function reportNoTarget(eventId: string, note: string): void {
  recordVerdict(eventId, false);
  showToast(note, 'warning');
}

/** Open an event wherever it lives, given nothing but its id.
 *
 *  The *event wake* card's jump is the caller. A wait exists precisely because
 *  the thread is watching for something happening SOMEWHERE ELSE, so the matched
 *  event is normally in another thread (in practice a `CodingAgentIdled` from
 *  the coding-agent thread the chat was watching), and `scrollToEventAndPulse`
 *  searches only the open thread's DOM. This resolves the owning thread first,
 *  then hands off to the ordinary deep-link path.
 *
 *  It shares `resolveEventTarget` with the render-time question the row asks, so
 *  a link that is DRAWN and a link that is FOLLOWED can never disagree about
 *  where the event lives. What this adds is the expensive half the render path
 *  refuses: loading the destination thread's events so its anchor can be
 *  computed at all.
 *
 *  Every path ends in a landing or a toast: a control whose whole job is "take
 *  me there" must never be a dead tap. */
export async function showEventWhereItLives(eventId: string): Promise<void> {
  if (!eventId) return;
  try {
    const target = await resolveEventTarget(eventId);
    if (target.kind === 'nowhere') {
      reportNoTarget(eventId, target.note);
      return;
    }
    if (target.kind === 'anchored') {
      focusThread(target.threadId, { targetEventId: target.anchor });
      return;
    }
    if (!(await ensureThreadByIdInMap(target.threadId))) {
      reportNoTarget(eventId, 'The thread that event happened in no longer exists.');
      return;
    }
    // The anchor is computed from the thread's OWN exchanges, so its events have
    // to be here first.
    await awaitThreadEvents(target.threadId);
    const anchor = anchorInLoadedThread(target.threadId, eventId);
    if (anchor) {
      focusThread(target.threadId, { targetEventId: anchor });
      return;
    }
    if (loadedThreadHolds(target.threadId, eventId)) {
      // The thread is here, it has the event, and it draws nothing for it. As
      // definite as the `nowhere` above and reported the same way. Navigating
      // with the raw id instead would spend the whole resolve deadline on an
      // element that is not in the DOM and cannot be.
      //
      // This is also the ONE place the render path's optimism gets corrected:
      // it answered `unloaded` for this event without loading the thread, and
      // the load has just proved there is nothing to land on.
      reportNoTarget(eventId, NOTHING_TO_LAND_ON);
      return;
    }
    // The event is not in the thread's events, either because they never
    // arrived or because the thread has not caught up with it. Falling back to
    // the raw id rather than bailing: it may still land, and
    // `scrollToEventAndPulse` waits out its own resolve deadline and reports for
    // itself if the target never renders.
    focusThread(target.threadId, { targetEventId: eventId });
  } catch (e) {
    if (e instanceof ApiError && e.httpCode === 404) {
      reportNoTarget(eventId, 'That event is no longer in the event store.');
      return;
    }
    // NOT `reportNoTarget`: a dropped connection is not an answer about the
    // event, so the affordance stays and the next tap can succeed.
    showToast(`Could not open that event: ${errorDetail(e)}`, 'error');
  }
}

/** Resolve once `threadId`'s events are in the store, it has given up loading
 *  them, or the wait times out.
 *
 *  Awaiting `loadThreadEvents` alone is NOT enough, and the gap is exactly where
 *  this feature would break. That function does not join an in-flight load: it
 *  early-returns on its `loadingThreads` guard, so while `loadAllThreads`'s eager
 *  pass is already fetching this thread the await resolves instantly against an
 *  empty event map. `deepLinkAnchorForEvent` would then find no turn holding the
 *  event and report no anchor, which is the one thing that cannot be true for an
 *  unstamped event like `CodingAgentIdled`. So: start a load if none is running,
 *  then wait on the store rather than on the call.
 *
 *  The budget is `EVENT_RESOLVE_DEADLINE_MS`, shared with the deep-link's own
 *  resolve rather than restated: both are waiting out the same lazily-loading
 *  thread. Timing out is not an error. The caller navigates with the raw id, and
 *  `scrollToEventAndPulse` owns the reporting from there. */
async function awaitThreadEvents(threadId: string): Promise<void> {
  await loadThreadEvents(threadId);
  const settledAlready = threadMap.value.get(threadId)?.eventsLoaded;
  if (settledAlready) return;
  await new Promise<void>(resolve => {
    let dispose: (() => void) | undefined;
    let settled = false;
    const finish = () => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      // Undefined when the effect settled on its own first synchronous run;
      // the `if (settled)` below disposes it once the assignment lands.
      dispose?.();
      resolve();
    };
    const timer = setTimeout(finish, EVENT_RESOLVE_DEADLINE_MS);
    dispose = effect(() => {
      const thread = threadMap.value.get(threadId);
      // A failed load settles too: waiting out the full deadline for events
      // that are not coming just delays the navigation.
      if (thread?.eventsLoaded || thread?.eventsLoadFailed) finish();
    });
    if (settled) dispose();
  });
}
