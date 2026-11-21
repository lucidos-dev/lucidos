/** Where a widget frame stands on its way to being seen.
 *
 *  - `unknown`: the viewport observer has not reported yet.
 *  - `off-screen`: out of view, so it holds nothing back.
 *  - `loading`: in view, and its cover has not lifted.
 *  - `revealed`: in view, and its cover has lifted. */
export type FrameState = 'unknown' | 'off-screen' | 'loading' | 'revealed';

/** `inView` is null until the viewport observer has answered for the frame's
 *  current visibility. `revealed` is the cover having lifted on the frame's
 *  current document. */
export function widgetFrameState(inView: boolean | null, showing: boolean, revealed: boolean): FrameState {
  if (inView === null) return 'unknown';
  if (revealed) return 'revealed';
  return showing ? 'loading' : 'off-screen';
}
