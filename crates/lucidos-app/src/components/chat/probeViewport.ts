/** The viewport reading both composer probes carry in every line.
 *
 *  Shared so a press episode and a typing episode are read side by side, from
 *  one set of numbers rather than two that drifted. A leaf module: it imports
 *  nothing, so either probe can take it without taking the other.
 *
 *  Both probes are diagnostics, registered in `docs/temporary-measures.md` § 1,
 *  and this goes when the last of them does. */

export interface ProbeViewport {
  vvHeight: number;
  vvOffsetTop: number;
  innerHeight: number;
  appHeight: string;
  keyboardActive: boolean;
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
