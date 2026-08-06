import { effect } from '@preact/signals';
import { ApiError } from '../../api/client';
import { fetchEventLocation } from '../../api/threads';
import { EVENT_RESOLVE_DEADLINE_MS } from '../../components/chat/scrollState';
import { computeExchanges, deepLinkAnchorForEvent } from '../thread-events';
import { showToast, threadMap, focusedThreadId } from '../store';
import { errorDetail } from '../../utils/errorDetail';
import { ensureThreadByIdInMap, loadThreadEvents } from './thread-loading';
import { focusThread } from './threads';

/** Open an event wherever it lives, given nothing but its id.
 *
 *  The *event wait* step's "show it" is the caller. A wait exists precisely
 *  because the thread is watching for something happening SOMEWHERE ELSE, so
 *  the matched event is normally in another thread (in practice a
 *  `CodingAgentIdled` from the coding-agent thread the chat was watching), and
 *  `scrollToEventAndPulse` searches only the open thread's DOM. This resolves
 *  the owning thread first, then hands off to the ordinary deep-link path.
 *
 *  Two resolutions happen here, and both are needed for the link to land:
 *
 *   - **Which thread.** `EventWaitDelivered` carries the matched event's id,
 *     type and payload but not its thread, so `GET /events/:id/location`
 *     answers that. A `null` thread id is a real answer (a workspace domain
 *     event belongs to no conversation), distinct from the 404 an unknown id
 *     returns, and the two get different words.
 *   - **Which element.** Only an exchange starter and a `ResponseFailed` card
 *     carry `data-event-id`, so a matched step would miss in the destination
 *     thread exactly as it missed in the source one. `deepLinkAnchorForEvent`
 *     re-targets it at the turn that contains it, which needs the thread's
 *     events in the store first, hence the `awaitThreadEvents` before focusing.
 *
 *  Every path ends in a landing or a toast: a button whose whole job is "take
 *  me there" must never be a dead tap. */
export async function showEventWhereItLives(eventId: string): Promise<void> {
  if (!eventId) return;
  try {
    const threadId = await owningThreadId(eventId);
    if (!threadId) {
      showToast(
        'That event is a workspace event, not part of any conversation, so there is nowhere to open it.',
        'warning',
      );
      return;
    }
    if (!(await ensureThreadByIdInMap(threadId))) {
      showToast('The thread that event happened in no longer exists.', 'warning');
      return;
    }
    // The anchor is computed from the thread's OWN exchanges, so its events have
    // to be here first.
    await awaitThreadEvents(threadId);
    const thread = threadMap.value.get(threadId);
    const anchor = thread
      ? deepLinkAnchorForEvent(computeExchanges(thread), eventId)
      : null;
    // Falling back to the raw id rather than bailing: the events may still be
    // arriving, and `scrollToEventAndPulse` waits out its own resolve deadline
    // and reports for itself if the target never renders.
    focusThread(threadId, { targetEventId: anchor ?? eventId });
  } catch (e) {
    if (e instanceof ApiError && e.httpCode === 404) {
      showToast('That event is no longer in the event store.', 'warning');
      return;
    }
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
 *  event and fall back to the raw id, which is the one thing that cannot resolve
 *  for an unstamped event like `CodingAgentIdled`. So: start a load if none is
 *  running, then wait on the store rather than on the call.
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

/** The thread holding `eventId`, or `null` when it belongs to none.
 *
 *  Checks the focused thread's already-loaded events before asking the engine.
 *  A wait can match an event in its OWN thread, and on an iOS PWA over Tailscale
 *  a round-trip is 400 to 800 ms steady state, which is long enough for a tap to
 *  read as dead. Only the focused thread is scanned: it is the one the user is
 *  looking at, and sweeping every loaded thread's event map would trade a
 *  bounded fetch for an unbounded walk. */
async function owningThreadId(eventId: string): Promise<string | null> {
  const openId = focusedThreadId.value;
  const open = openId ? threadMap.value.get(openId) : null;
  if (open) {
    for (const event of open.events.values()) {
      if (event._eventId === eventId) return openId;
    }
  }
  return (await fetchEventLocation(eventId)).thread_id;
}
