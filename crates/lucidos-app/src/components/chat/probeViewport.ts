/** The viewport reading both composer probes carry in every line.
 *
 *  Shared so a press episode and a typing episode are read side by side, from
 *  one set of numbers rather than two that drifted. A leaf module: it imports
 *  nothing, so either probe can take it without taking the other.
 *
 *  Both probes are diagnostics, registered in `docs/temporary-measures.md` § 1,
 *  and this goes when the last of them does. */

/** The state that separates a layout fault from an event fault. Carried in
 *  every report, because the reporter is a phone with a screenshot.
 *
 *  `keyboardActive` is the `data-keyboard-active` flag on `<html>`. A whole
 *  block of `styles/mobile.css` inerts the header, the title row, the
 *  edge-swipe zones and the transcript's children off it, and re-enables
 *  `.prompt-area`. A flag out of step with the keyboard would read exactly like
 *  the bug being chased. */
export interface ProbeViewport {
  vvHeight: number;
  vvOffsetTop: number;
  innerHeight: number;
  appHeight: string;
  keyboardActive: boolean;
  /** `window.scrollY`. A LAYOUT viewport scrolled under a fixed shell is the
   *  textbook form of this bug on iOS, and no report has carried the number. */
  pageScrollY: number;
}

export function readViewport(): ProbeViewport {
  const vv = window.visualViewport;
  return {
    vvHeight: vv?.height ?? window.innerHeight,
    vvOffsetTop: vv?.offsetTop ?? 0,
    innerHeight: window.innerHeight,
    appHeight: document.documentElement.style.getPropertyValue('--app-height'),
    keyboardActive: document.documentElement.hasAttribute('data-keyboard-active'),
    pageScrollY: window.scrollY,
  };
}
