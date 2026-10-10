import { effect } from '@preact/signals';
import { focusedThreadId, threadsLoaded } from '../store';
import { homeThreadId } from './homeThread';
import { focusThread } from './threads';

/** Which thread a cold start opens, or `null` to leave the focus alone.
 *
 *  Home, when this device stored no focus and nothing was focused while the
 *  thread list loaded (ADR 0411). A stored focus is a returning user's last
 *  thread, and it wins. So does anything the user opened or typed meanwhile.
 *  Pure, so the rule is testable without booting the client. */
export function coldStartFocus(opts: {
  storedFocus: boolean;
  focusedNow: string | null;
  homeId: string | null;
}): string | null {
  if (opts.storedFocus || opts.focusedNow) return null;
  return opts.homeId;
}

/** Open Home once, as the thread list first lands, if {@link coldStartFocus}
 *  says so. `storedFocus` is whether this device had a focus stored at boot,
 *  read before anything could change it. Returns the stop function. */
export function openHomeOnColdStart(storedFocus: boolean): () => void {
  let decided = false;
  const stop = effect(() => {
    if (decided || !threadsLoaded.value) return;
    decided = true;
    const target = coldStartFocus({
      storedFocus,
      focusedNow: focusedThreadId.peek(),
      homeId: homeThreadId.peek(),
    });
    // Bookkeeping, not a navigation: the cold start already shows the thread
    // pane, and a reveal would swipe a phone that opened somewhere else.
    if (target) focusThread(target, { revealPane: false });
  });
  return stop;
}
