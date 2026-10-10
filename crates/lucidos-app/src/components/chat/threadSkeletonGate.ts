/** When the thread transcript shows its loading skeleton.
 *
 *  ONE clock, the shared `SPINNER_DELAY_MS` delay gate, exactly like every other
 *  loader in the app. A second clock lived here for one day: a hold that put the
 *  skeleton up AHEAD of the gate whenever a landing snapshot looked expensive to
 *  render. It was reverted the same day. Read ADR 0081 before bringing it back,
 *  which records what it cost and why the trade does not work. */

/** The terms ThreadView already derives per render, named so the gate below can
 *  be read without it. */
export interface ThreadLoadingState {
  hasExchanges: boolean;
  animating: boolean;
  eventsLoadFailed: boolean;
  eventsLoaded: boolean;
  disconnected: boolean;
}

/** Is the transcript mid-open, with nothing to show and nothing terminal to
 *  say? Mirrors `emptyReason`'s `loading` verdict. Also valid before the thread
 *  reaches `threadMap`, where every flag reads false. */
export function threadIsLoadingNow(state: ThreadLoadingState): boolean {
  return !state.hasExchanges
    && !state.animating
    && !state.eventsLoadFailed
    && !state.eventsLoaded
    && !state.disconnected;
}

/** Does the transcript paint its skeleton this render?
 *
 *  Only while loading, and only once the delay gate has elapsed. A terminal
 *  state always wins, so the skeleton can never cover "No messages in this
 *  thread" or an unreachable workspace. */
export function showThreadSkeletonNow(
  state: ThreadLoadingState,
  delayElapsed: boolean,
): boolean {
  return threadIsLoadingNow(state) && delayElapsed;
}
