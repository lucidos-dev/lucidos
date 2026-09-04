import { useState, useEffect } from 'preact/hooks';
import { getActiveScrollElement, isElementOnScreen } from '../components/chat/scrollState';

/** Is `el` inside the transcript's visible band, kept current as the reader
 *  scrolls?
 *
 *  Named for the transcript because that is the only band it can answer for.
 *  The verdict is `isElementOnScreen`, which measures against the ACTIVE SCROLL
 *  ELEMENT, and only the transcript ever registers as one. An element outside
 *  it is measured against a box it does not sit in.
 *
 *  It seeds `true` and holds its last answer while `el` is `null`, rather than
 *  resetting. Today's caller passes the live step row, which a working turn
 *  replaces about once per action. Resetting on each swap would flash the
 *  affordance the caller is deciding about. So a caller that needs "no element"
 *  to mean something must say so itself, and `ChatExchange` does. */
export function useOnScreenInTranscript(el: HTMLElement | null): boolean {
  const [onScreen, setOnScreen] = useState(true);
  // The root is read once, at arm time, and nothing about it is reactive. A
  // transcript that registers LATE would leave a window-rooted observer
  // measuring a different band from the verdict, and the answer would latch.
  // Re-reading it here re-arms on the next render instead.
  const root = getActiveScrollElement();
  useEffect(() => (el ? watchOnScreen(el, setOnScreen) : undefined), [el, root]);
  return onScreen;
}

/** Watch `el`, reporting through `onChange` until the returned teardown runs.
 *  Exported for its own test: the suite has no jsdom, so a hook cannot be
 *  rendered, and this is the whole of what the hook does.
 *
 *  The `IntersectionObserver` is the CHANGE NOTIFIER only, never the verdict:
 *  `isElementOnScreen` owns the band, so one definition serves this, the *seen
 *  target* rule, and choice-card navigation. It reports once up front, so a
 *  target that never crosses the root is still answered.
 *
 *  One observer is the ONLY notifier here, which is what makes the root matter
 *  so much. `notification-visit.ts` roots its own at the window and gets away
 *  with it, because a scroll, a navigation and a mutation each resample it
 *  too. */
export function watchOnScreen(el: HTMLElement, onChange: (onScreen: boolean) => void): () => void {
  const sample = () => onChange(isElementOnScreen(el));
  sample();
  if (typeof IntersectionObserver !== 'function') return () => {};
  // The verdict's own reference box, so the two cannot measure different bands.
  // The window would be the wrong root. The header and the prompt region inset
  // the transcript, so a row leaving it is still well inside the window. That
  // crossing is the one this exists to catch. It is also why the element has to
  // BE in the transcript. A root that does not contain its target intersects it
  // emptily forever, and the answer then sticks.
  const observer = new IntersectionObserver(sample, { root: getActiveScrollElement() });
  observer.observe(el);
  return () => observer.disconnect();
}
