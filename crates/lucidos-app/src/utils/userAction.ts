/** "The user did something", as distinct from anything the app did on its own.
 *
 *  One definition, shared by every surface that must stand down once the user
 *  takes over: the navigation focus marker fades on it
 *  (`components/shared/focusMarker.ts`), and a notification deep-link that gave
 *  up waiting for its target suppresses its fallback scroll on it
 *  (`components/chat/scrollState.ts`), so a user who scrolled away to read
 *  history is never yanked by a late correction.
 *
 *  **"Did the user SCROLL" is a narrower question and has its own answer**, in
 *  `scrollState`'s `readerGestureActive`. It cannot use this set, because
 *  `pointerdown` is in it: a press is how the reader answers a question card or
 *  grants a permission, and both must leave a *standing follow* armed. It also
 *  needs the element the gesture landed on, where these consumers only need
 *  that one happened, and a freshness window (`USER_SCROLL_WINDOW_MS`,
 *  `utils/scrollActivity.ts`) where these are one-shot callbacks. What it does
 *  keep is the property below, which is what makes any of these reliable. */

/** The input events that count as a user action. `wheel` / `touchmove` cover
 *  scrolling, `pointerdown` covers clicks, taps and a scrollbar drag, `keydown`
 *  covers any keypress (keyboard scrolling included). What makes the set a
 *  reliable signal is what is NOT in it: a programmatic `scrollTop` write, a
 *  `scrollIntoView`, a focus move and a re-render emit none of these, so the app
 *  can never mistake its own work for the user's. */
export const USER_ACTION_EVENTS = ['wheel', 'touchmove', 'pointerdown', 'keydown'] as const;

/** Call `handler` on every user action until the returned teardown runs
 *  (callers that want one-shot semantics tear down from inside the handler).
 *  Capture phase, so an action counts wherever it lands; passive, because no
 *  listener here ever calls preventDefault. Outside a DOM this is a no-op and
 *  the teardown is a no-op too, so callers need no environment check. */
export function watchUserAction(handler: () => void): () => void {
  if (typeof document === 'undefined' || !document.addEventListener) return () => {};
  for (const type of USER_ACTION_EVENTS) {
    document.addEventListener(type, handler, { capture: true, passive: true });
  }
  return () => {
    for (const type of USER_ACTION_EVENTS) {
      document.removeEventListener(type, handler, { capture: true } as EventListenerOptions);
    }
  };
}
