import { isIOS, isWebKit } from './platform';

/** Live action buttons whose tap mutates state irreversibly — answering a CC
 *  question (`.question-option`) or granting/denying a permission (the
 *  buttons inside `.permission-actions`). Resolved cards are safe: answered
 *  recaps render as a non-button class (`.question-option-static`), and
 *  terminated cards keep `.question-option` but are `disabled`, so they emit
 *  no click for the capture listener to swallow. */
export const IRREVERSIBLE_TAP_SELECTOR = '.question-option, .permission-actions button';

/** True when a click landed on (or inside) an irreversible action button. */
export function isIrreversibleTapTarget(target: EventTarget | null): boolean {
  const el = target as { closest?: (s: string) => unknown } | null;
  return !!(el && typeof el.closest === 'function' && el.closest(IRREVERSIBLE_TAP_SELECTOR));
}

export type ResumeEvent =
  | { kind: 'resume' }
  | { kind: 'settled' }
  | { kind: 'click'; irreversible: boolean };

export interface ResumeEffect {
  /** Cancel the originating DOM event so the irreversible action never fires. */
  swallow: boolean;
  /** Ask subscribers to force an iOS repaint of their content. */
  repaint: boolean;
}

/** Pure state machine behind the iOS-PWA resume guard.
 *
 *  iOS blanks the backing texture of a composited layer while the PWA is
 *  backgrounded: on resume the thread content is still in the DOM but renders
 *  black, and only a real interaction (or a forced repaint) brings it back. The
 *  first tap after a resume is therefore a "wake the screen" gesture, NOT a
 *  deliberate action — honoring it would answer a question / grant a permission
 *  the user cannot even see (the reported bug).
 *
 *  - `resume` arms the guard and asks for a proactive repaint.
 *  - `settled` fires once the post-resume repaint has completed (the page has
 *    painted the post-resume frames, so the content is visible again). It
 *    disarms the guard so the user's FIRST deliberate tap on the now-visible
 *    card is honored — the earlier design swallowed it unconditionally, which is
 *    the "first click after coming into the app does not register" report.
 *  - a `click` while armed (i.e. before `settled`) disarms and repaints; it is
 *    swallowed only if it hit an irreversible action button. A benign first tap
 *    just disarms — the user's next tap is honored normally.
 *
 *  The guard window is therefore "resume → content repainted", not "resume →
 *  first tap": the swallow only bites the genuine wake-tap that lands while the
 *  layer might still be black. Because the settle is driven by animation frames
 *  (which only advance while the page actually paints), a slow iOS repaint keeps
 *  the guard armed for exactly as long as the content could still be black, then
 *  releases it the instant the layer comes back.
 *
 *  Note we deliberately do NOT disarm on scroll: a bfcache (`pageshow`) restore
 *  can fire a scroll-position-restore event right after the resume, which would
 *  disarm before the wake-tap and let it answer the invisible question. Erring
 *  toward swallowing one extra tap is safe; erring toward answering is the bug. */
export function reduceResumeGuard(
  armed: boolean,
  ev: ResumeEvent,
): { armed: boolean; effect: ResumeEffect } {
  switch (ev.kind) {
    case 'resume':
      return { armed: true, effect: { swallow: false, repaint: true } };
    case 'settled':
      return { armed: false, effect: { swallow: false, repaint: false } };
    case 'click':
      if (!armed) return { armed: false, effect: { swallow: false, repaint: false } };
      return { armed: false, effect: { swallow: ev.irreversible, repaint: true } };
  }
}

// --- DOM wiring (WebKit, with an iOS-only wake-tap guard) ------------------

const repaintSubscribers = new Set<() => void>();
let armed = false;
let installed = false;
let settleScheduled = false;

function fireRepaint(): void {
  for (const cb of repaintSubscribers) cb();
}

function step(ev: ResumeEvent, domEvent?: Event): void {
  const { armed: next, effect } = reduceResumeGuard(armed, ev);
  armed = next;
  if (effect.repaint) fireRepaint();
  if (effect.swallow && domEvent) {
    domEvent.stopPropagation();
    domEvent.preventDefault();
  }
}

/** Disarm once the post-resume repaint has painted. `forceWebKitRepaint` un-blanks
 *  the layer across a double rAF, so we wait the same span: by the time the
 *  second frame runs the content is visible and any tap is deliberate. rAF only
 *  advances while the page actually renders, so a stalled iOS repaint defers the
 *  settle in lockstep — the guard stays armed for exactly the window the layer
 *  could still be black. Coalesced so the visibilitychange+pageshow+focus burst
 *  a single resume fires schedules only one settle. */
function scheduleSettle(): void {
  if (settleScheduled || typeof requestAnimationFrame !== 'function') return;
  settleScheduled = true;
  requestAnimationFrame(() =>
    requestAnimationFrame(() => {
      settleScheduled = false;
      step({ kind: 'settled' });
    }),
  );
}

function onResume(): void {
  step({ kind: 'resume' });
  scheduleSettle();
}
function onVisibilityChange(): void {
  if (document.visibilityState === 'visible') onResume();
}
function onClickCapture(e: MouseEvent): void {
  step({ kind: 'click', irreversible: isIrreversibleTapTarget(e.target) }, e);
}

function install(): void {
  if (installed || !isWebKit()) return;
  installed = true;
  // Match useStartup's canonical resume set — iOS often restores a PWA via
  // `pageshow` (bfcache) or `focus` with no `visible` `visibilitychange`, which
  // left the old visibilitychange-only repaint silent and the content black
  // until a tap.
  document.addEventListener('visibilitychange', onVisibilityChange);
  window.addEventListener('pageshow', onResume);
  window.addEventListener('focus', onResume);
  // The wake-tap guard is iOS ONLY, unlike the repaint above. A desktop window
  // is raised BY a click, so swallowing the first one there would eat an
  // ordinary click-to-focus. The phone has no such gesture: its first tap
  // arrives on a layer that may still be blank.
  //
  // Capture phase so the wake-tap is swallowed before it reaches the option /
  // permission button's own onClick — the same swallow pattern as
  // useAnchoredPopover's outside-click handler.
  if (isIOS()) document.addEventListener('click', onClickCapture, true);
}

/** Subscribe a callback fired whenever the page returns to the foreground (and
 *  on the first wake-tap after one) so the caller can force a WebKit repaint.
 *  Lazily installs the resume listeners. Returns an unsubscribe.
 *
 *  No-op off WebKit, since the compositor bug is that engine's. The wake-tap
 *  swallow inside it stays narrower still, at iOS alone: see `install`. */
export function onPageResume(cb: () => void): () => void {
  install();
  repaintSubscribers.add(cb);
  return () => {
    repaintSubscribers.delete(cb);
  };
}
