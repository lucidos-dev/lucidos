/** jsdom implements neither font loading nor layout, so it ships no FontFaceSet
 *  and no ResizeObserver. The trigger form's Intent textarea subscribes to both
 *  (`useFontMetricsResize`, `useWidthRemeasure`), and an absent global throws at
 *  mount instead of degrading. Neither stub ever fires here: no font loads, and
 *  nothing has a width to change. */
export function stubIntentFieldObservers(): void {
  if (!('fonts' in document)) {
    Object.defineProperty(document, 'fonts', {
      value: { addEventListener() {}, removeEventListener() {} },
    });
  }
  if (typeof globalThis.ResizeObserver === 'undefined') {
    globalThis.ResizeObserver = class {
      observe() {}
      unobserve() {}
      disconnect() {}
    } as unknown as typeof ResizeObserver;
  }
}
