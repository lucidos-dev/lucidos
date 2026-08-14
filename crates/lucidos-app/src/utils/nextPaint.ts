/** Resolve once the browser has PAINTED whatever is currently committed.
 *
 *  Two nested frame callbacks, the standard idiom. The first runs before the
 *  pending frame is painted; the second runs on the frame after it. One callback
 *  alone resolves before the pixels land, which defeats every caller.
 *
 *  Reads the callback off `globalThis` per call, so a test can drive the frames,
 *  and resolves at once where there is none. */
export function nextPaint(): Promise<void> {
  const raf = globalThis.requestAnimationFrame;
  if (typeof raf !== 'function') return Promise.resolve();
  return new Promise<void>(resolve => {
    raf(() => raf(() => resolve()));
  });
}
