import { connectionStatus, removeToast, showToast, THREAD_LIST_REFRESH_TOAST_KEY } from '../store';
import { isTransientFetchError } from '../../api/client';
import { errorDetail } from '../../utils/errorDetail';
import { loadAllThreads } from './thread-loading';

/** Refresh the thread list at a sync point, and own the whole outcome: the one
 *  keyed card, the rule for when it is honest, and its retraction.
 *
 *  Two callers, and they are the reason this is a wrapper rather than a
 *  `report(err)` helper the two of them call from their own catch blocks: a
 *  resume (`runResumeSync` in connection.ts) and an SSE reconnect / `Lagged`
 *  resync (`resyncLoadedThreads` in thread-sync.ts) report the SAME failure of
 *  the SAME request through the SAME toast key, so a divergence between them
 *  would show up only during an outage and only in one direction. Owning both
 *  outcome paths here makes that impossible to get wrong, and it is also the
 *  only shape that can retract the card, since a retraction lives on the success
 *  path where neither caller had one.
 *
 *  Its own module rather than `thread-loading.ts`, where the sibling helpers for
 *  the two per-thread event surfaces live: four existing suites mock
 *  `./thread-loading` with an explicit export list, and Vitest throws on a named
 *  import a mock factory does not define, so landing one function there would
 *  mean editing four unrelated mocks.
 *
 *  Never rejects. `loadAllThreads` is the only thing that can, and every way it
 *  can is answered below.
 *
 *  Deliberately NOT used by the third `loadAllThreads` call site, the
 *  `!threadsLoaded` cold-start retry in `checkConnection`. That one recovers an
 *  initial load that never landed rather than refreshing a list already on
 *  screen, and `ThreadView` paints its failure itself. */
export async function refreshThreadList(): Promise<void> {
  let landed: boolean;
  try {
    landed = await loadAllThreads();
  } catch (err) {
    reportThreadListRefreshFailure(err);
    return;
  }
  // `false` means the load DECLINED: the engine is mid-restart, or another load
  // is already in flight and this call did not wait for it. Either way nothing
  // was read here and nothing is proven, so retracting would clear a card that
  // is still true (and, in the in-flight case, hide the failure the other load
  // is about to report). This is why `loadAllThreads` answers with a boolean
  // rather than just resolving: a resolved promise alone cannot be read as "the
  // list is fresh".
  if (!landed) return;
  // The card asserts that this device's thread list is behind, which a landed
  // refresh has just made false. `removeToast`, not `dismissToast`: this is a
  // signal-driven hide, not the user dismissing anything. A no-op when nothing
  // is showing, which is the ordinary case.
  removeToast(THREAD_LIST_REFRESH_TOAST_KEY);
}

/** Decide what a rejected refresh is worth telling the user, in three branches.
 *
 *  A transient rejection says nothing about the engine, so it says nothing to
 *  the user. `isTransientFetchError` folds `TimeoutError` in with the aborts and
 *  the transport errors, and folding it in is a deliberate reversal of the
 *  narrower predicate this site used to carry (see `utils/errorDetail.ts`, where
 *  the superseded argument stood). The reasoning that changed: over a dropped
 *  tunnel the GET hangs rather than refusing, so the client deadline fires and a
 *  timeout becomes the ordinary SHAPE of a link outage rather than evidence the
 *  engine went quiet. Measured on 2026-08-07, the engine answered every
 *  `/api/v1/threads` in the reported window between 12ms and 2s against a 10s
 *  client deadline, so the card the user saw was reporting the link and blaming
 *  the engine. No toast is right here because the connection dot already owns a
 *  sustained outage, the next sync point re-syncs on its own, and the drawer
 *  keeps rendering the metadata it already had in the meantime.
 *
 *  Nothing is said while the engine is unreachable either, for the reason the
 *  two per-thread event surfaces and `loadUnreadNotifications` already stand
 *  down: an outage is reported once, by the dot. The dot's hysteresis keeps it
 *  green through a brief blip, so a verdict during one is still surfaced.
 *
 *  Anything else is a verdict: the engine answered and refused (an `ApiError`),
 *  or the answer was unusable (a parse error). `.claude/rules/frontend.md`
 *  § No Hidden Errors requires that reach the user, and none of the reasoning
 *  above touches it. */
function reportThreadListRefreshFailure(err: unknown): void {
  if (isTransientFetchError(err)) {
    console.warn('[ThreadList] refresh failed transiently; the next sync point re-syncs', err);
    return;
  }
  if (connectionStatus.value !== 'connected') return;
  showToast(`Failed to refresh the thread list: ${errorDetail(err)}`, 'error', {
    key: THREAD_LIST_REFRESH_TOAST_KEY,
  });
}
