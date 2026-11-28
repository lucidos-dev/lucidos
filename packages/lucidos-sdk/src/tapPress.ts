
import { LONG_PRESS_DELAY_MS } from './widgetFrame';

export const TAP_MOVE_THRESHOLD_PX = 8;

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
