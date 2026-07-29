// Movement during a press that is treated as a scroll instead of a tap.
// 8 px matches the swipe direction-lock threshold in `swipe.ts`.
const TAP_MOVE_THRESHOLD_PX = 8;

interface TapState {
  startX: number;
  startY: number;
  canceled: boolean;
}

/** Tap-vs-scroll gate for touch-friendly action buttons whose `click` would
 *  trigger an irreversible side effect (e.g. answering a CC question). iOS
 *  Safari fires `click` after a brief touch even if the user was starting a
 *  scroll — when the movement stays under iOS's ~10 px native cancel
 *  threshold — so an accidental swipe registers as a press.
 *
 *  Wire `down`/`move`/`cancel` to `onPointerDown`/`onPointerMove`/
 *  `onPointerCancel`, and call `isTap()` inside `onClick`. */
export function createTapGate() {
  let state: TapState | null = null;
  return {
    down(x: number, y: number): void {
      state = { startX: x, startY: y, canceled: false };
    },
    move(x: number, y: number): void {
      if (!state || state.canceled) return;
      if (
        Math.abs(x - state.startX) > TAP_MOVE_THRESHOLD_PX ||
        Math.abs(y - state.startY) > TAP_MOVE_THRESHOLD_PX
      ) {
        state.canceled = true;
      }
    },
    cancel(): void {
      state = null;
    },
    /** True when no press was recorded (keyboard / programmatic click) or
     *  when the press's movement stayed within the threshold. Resets state so
     *  the next press starts fresh — without this a stale "moved" flag from
     *  one press could suppress the next click. */
    isTap(): boolean {
      const ok = state === null || !state.canceled;
      state = null;
      return ok;
    },
  };
}
