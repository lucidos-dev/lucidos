/** True if the element is actually visible: not `display: none`, and not
 *  clipped by a zero-height `overflow: hidden` ancestor. That second case is
 *  why the walk below exists, since such an ancestor keeps its
 *  `position: fixed` children rendering and leaves the element measuring
 *  full size. */
export function isElementVisible(el: HTMLElement): boolean {
  return unmeasurableBox(el) === null;
}

/** WHICH box makes `el` unmeasurable: `el` itself, or the nearest ancestor
 *  clipping it to nothing. `null` when `el` is measurable, which is what
 *  `isElementVisible` reports above.
 *
 *  Naming the box rather than answering yes or no is what lets a caller WAIT
 *  for it. A box is the only thing a `ResizeObserver` can watch, and the two
 *  answers point at DIFFERENT elements. A collapsed pane zeroes the target's
 *  own rect. An ancestor clipped to nothing leaves the target measuring full
 *  size while showing none of it. See `watchForABox` in `scrollState.ts`. */
export function unmeasurableBox(el: HTMLElement): HTMLElement | null {
  const r = el.getBoundingClientRect();
  if (r.width <= 0 || r.height <= 0) return el;
  // An element inside a zero-height overflow:hidden container reports non-zero
  // dimensions from layout, so the clipping ancestor has to be found by walking.
  let ancestor = el.parentElement;
  while (ancestor && ancestor !== document.documentElement) {
    // display:contents removes the element's own box, so it measures 0x0 while
    // its children are fully visible. Skip these ancestors.
    if (getComputedStyle(ancestor).display === 'contents') {
      ancestor = ancestor.parentElement;
      continue;
    }
    const ar = ancestor.getBoundingClientRect();
    if (ar.height <= 0 || ar.width <= 0) return ancestor;
    ancestor = ancestor.parentElement;
  }
  return null;
}
