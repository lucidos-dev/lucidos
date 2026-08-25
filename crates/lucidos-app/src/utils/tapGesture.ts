// Movement during a press that is treated as a scroll instead of a tap.
// 8 px matches the swipe direction-lock threshold in `swipe.ts`.
const TAP_MOVE_THRESHOLD_PX = 8;

/** The pointer fields the gate reads: SCREEN coordinates, never client ones.
 *
 *  The gate takes the event rather than two numbers so that no call site can
 *  hand it another coordinate space. `clientX/clientY` measure the finger
 *  against the page viewport, so they move when the viewport moves. With the
 *  iOS keyboard up the visual viewport is offset and settles under a
 *  stationary finger. The gate read that as a swipe and discarded a real tap
 *  on the composer's Submit.
 *
 *  Client coordinates are wrong in the other direction too. A scrolling page
 *  travels with the finger, so `clientY` barely changes across a gesture that
 *  crossed the screen. Screen coordinates are what "did the finger move"
 *  actually means. */
export interface TapPointer {
  screenX: number;
  screenY: number;
}

interface TapState {
  startX: number;
  startY: number;
  /** Largest per-axis movement seen so far. A discarded tap reports it, so a
   *  button that swallowed a press can say how far the press travelled. */
  movedPx: number;
  canceled: boolean;
}

/** Tap-vs-scroll gate for touch-friendly action buttons whose `click` would
 *  trigger an irreversible side effect (e.g. answering a CC question).
 *
 *  iOS Safari fires `click` after a brief touch even when the user was
 *  starting a scroll. That happens whenever the movement stays under iOS's
 *  ~10 px native cancel threshold, so an accidental swipe registers as a
 *  press.
 *
 *  Wire `down`/`move`/`cancel` to `onPointerDown`/`onPointerMove`/
 *  `onPointerCancel`, and call `isTap()` inside `onClick`. */
export function createTapGate() {
  let state: TapState | null = null;

  /** Consumes the in-flight press and rules on it. `null` when it counts as a
   *  tap, otherwise how far it travelled in screen px. Consuming is what stops
   *  a stale "moved" flag from one press suppressing the next click. */
  function settle(): number | null {
    const press = state;
    state = null;
    if (press === null || !press.canceled) return null;
    return Math.round(press.movedPx);
  }

  return {
    down(p: TapPointer): void {
      state = { startX: p.screenX, startY: p.screenY, movedPx: 0, canceled: false };
    },
    move(p: TapPointer): void {
      // Keeps measuring after the press is already doomed, because the
      // distance is what the discarded-tap toast reports. Stopping at the
      // threshold made every rejection read as a hair over 8 px.
      if (!state) return;
      state.movedPx = Math.max(
        state.movedPx,
        Math.abs(p.screenX - state.startX),
        Math.abs(p.screenY - state.startY),
      );
      if (state.movedPx > TAP_MOVE_THRESHOLD_PX) state.canceled = true;
    },
    cancel(): void {
      state = null;
    },
    /** True when no press was recorded (keyboard / programmatic click) or
     *  when the press's movement stayed within the threshold. */
    isTap(): boolean {
      return settle() === null;
    },
    /** `isTap()` for a caller that must SAY why it dropped the action. `null`
     *  when the press was a tap, otherwise the screen px it travelled. Each of
     *  the two consumes the press, so a caller uses one. */
    tapRejection(): number | null {
      return settle();
    },
  };
}

/** How long after a `touchend` a `click` counts as that touch's synthetic twin.
 *  Only the click path reads it, so a real second tap is never suppressed: the
 *  touch path serves every `touchend` it is given. */
const TOUCH_CLICK_WINDOW_MS = 500;

interface TouchTargetRect {
  left: number;
  right: number;
  top: number;
  bottom: number;
}

/** What the touch path reads off the event: the finger that lifted, and the box
 *  it had to lift inside. Structural, like `TapPointer` above, so the helper
 *  unit-tests without a DOM. */
export interface TouchEndLike {
  preventDefault(): void;
  changedTouches?: ArrayLike<{ clientX: number; clientY: number }>;
  currentTarget?: { getBoundingClientRect(): TouchTargetRect } | null;
}

/** Whether the finger lifted while still on the button.
 *
 *  A `touchend` is dispatched to the element the touch STARTED on, wherever it
 *  ends. A press that slid off therefore arrives here looking like a tap, and
 *  would activate where a `click` never would. Touch activation substitutes for
 *  the click. It must not be the looser trigger.
 *
 *  CLIENT coordinates, deliberately, unlike `createTapGate` right above. That
 *  measures whether the finger MOVED, across a span the viewport can shift
 *  under, so it needs screen space. This asks whether one point is inside one
 *  box, both read in the same frame, which is what client space answers.
 *
 *  True when the event cannot say, mirroring the gate's treatment of a click
 *  with no press behind it. A real browser always says. */
function liftedOnTarget(e: TouchEndLike): boolean {
  const point = e.changedTouches?.[0];
  const rect = e.currentTarget?.getBoundingClientRect();
  if (!point || !rect) return true;
  return point.clientX >= rect.left && point.clientX <= rect.right
    && point.clientY >= rect.top && point.clientY <= rect.bottom;
}

export interface TouchActivateOptions {
  /** Stands the TOUCH path down while it returns false, leaving the click path
   *  alone. A button whose action turns destructive in some state passes that
   *  state here, so the state keeps ordinary click activation. */
  enabled?: () => boolean;
  /** Injected clock, so the twin window is testable without a real one. */
  now?: () => number;
}

/** Activation handlers for a button whose action must survive the iOS keyboard.
 *
 *  Tapping a button while a text field is focused can blur the field. The
 *  keyboard then dismisses, the visual viewport shifts, and the button moves out
 *  from under the finger. WebKit drops the synthetic `click` it was about to
 *  dispatch, so the press reads as dead and the user taps again.
 *
 *  `onTouchEnd` runs the action inside the gesture, before any of that, and
 *  cancels the synthetic click. `onClick` serves the mouse, the keyboard and a
 *  programmatic click, and ignores the twin of a touch it already served.
 *
 *  One `action` for both paths on purpose: two callbacks would drift, and the
 *  touch path is the one nobody can exercise on a desktop. Compose with
 *  `createTapGate` by doing the gate check inside `action`, which keeps one
 *  press to one settle whichever path fires.
 *
 *  Local copies of this repair live in `promptFocus.ts` (`composeHandlers`,
 *  which adds focus-first) and `FileSearchModal.tsx`. */
export function touchActivated(action: () => void, opts: TouchActivateOptions = {}) {
  const enabled = opts.enabled ?? (() => true);
  const clock = opts.now ?? Date.now;
  let lastTouchAt: number | null = null;
  return {
    onTouchEnd(e: TouchEndLike): void {
      if (!enabled() || !liftedOnTarget(e)) return;
      lastTouchAt = clock();
      e.preventDefault();
      action();
    },
    onClick(): void {
      if (lastTouchAt !== null && clock() - lastTouchAt < TOUCH_CLICK_WINDOW_MS) {
        // A touch has ONE twin. Forget it here, so a genuine second tap landing
        // inside the window is served rather than eaten as well.
        lastTouchAt = null;
        return;
      }
      action();
    },
  };
}
