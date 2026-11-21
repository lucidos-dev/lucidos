/**
 * What counts as a tap, shared by the host's tap gates and the link press
 * guard, so one definition rules on every touch.
 */

import { LONG_PRESS_DELAY_MS } from './widgetFrame';

// Movement during a press that is treated as a scroll instead of a tap.
// 8 px matches the swipe direction-lock threshold in `paneSwipe.ts`.
// Exported for `deadPressProbe`, which asks the same question of a cancelled
// gesture: did the finger move, or did the system take a stationary press?
export const TAP_MOVE_THRESHOLD_PX = 8;

/** Was a press the user meant a tap?
 *
 *  One finger, start to end, that stayed within the tap threshold and was
 *  not held past a long press. A scroll, a pinch or a hold fails one of the
 *  three, and the system is entitled to take those. */
export function cancelledPressWasATap(p: {
  heldMs: number;
  movedPx: number;
  fingers: number;
  fingersLeft: number;
}): boolean {
  return p.fingers <= 1
    && p.fingersLeft === 0
    && p.movedPx <= TAP_MOVE_THRESHOLD_PX
    && p.heldMs <= LONG_PRESS_DELAY_MS;
}
