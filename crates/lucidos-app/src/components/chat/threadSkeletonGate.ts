
export function wholeHistoryLoaded(eventsLoaded: boolean, hasOlderEvents: boolean): boolean {
  return eventsLoaded && !hasOlderEvents;
}

export interface ThreadLoadingState {
  hasExchanges: boolean;
  animating: boolean;
  eventsLoadFailed: boolean;
  historyLoaded: boolean;
  disconnected: boolean;
}

export function threadIsLoadingNow(state: ThreadLoadingState): boolean {
  return !state.hasExchanges
    && !state.animating
    && !state.eventsLoadFailed
    && !state.historyLoaded
    && !state.disconnected;
}

export function showThreadSkeletonNow(
  state: ThreadLoadingState,
  delayElapsed: boolean,
): boolean {
  return threadIsLoadingNow(state) && delayElapsed;
}
