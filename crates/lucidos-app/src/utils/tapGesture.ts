// Movement during a press that is treated as a scroll instead of a tap.
// 8 px matches the swipe direction-lock threshold in `swipe.ts`.
const TAP_MOVE_THRESHOLD_PX = 8;

/** The fields everything in this file reads off a pointer or a touch: SCREEN
 *  coordinates, never client ones.
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

/** All the touch path needs off the event. Structural, like `TapPointer` above,
 *  so the helper unit-tests without a DOM. */
export interface TouchEventLike {
  preventDefault(): void;
}

/** A `createTapGate` as an activation sees it. Both halves travel together
 *  because both concern ONE press, and the gate holds one press at a time. */
export interface ActivationGate {
  /** Rule on the press and report a rejection. The CLICK path only asks. */
  pass(): boolean;
  /** Spend the press without ruling on it, for the TOUCH path, which serves it
   *  without asking. An unspent press is one the gate still holds. It would
   *  then rule on the NEXT activation that arrives with no press of its own,
   *  such as a keyboard Enter on the same button. */
  spend(): void;
}

export interface TouchActivateOptions {
  /** Stands the TOUCH path down while it returns false, leaving the click path
   *  alone. A button whose action turns destructive in some state passes that
   *  state here, so the state keeps ordinary click activation. */
  enabled?: () => boolean;
  /** The scroll-vs-tap gate. See `onClick` for why the touch path spends the
   *  press rather than asking. Absent means every click activates. */
  gate?: ActivationGate;
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
 *  programmatic click, and ignores the twin of a touch it already served. One
 *  `action` for both, since two callbacks would drift.
 *
 *  **The touch path takes every press it is given.** With a field focused on iOS
 *  it is the only path, so any test it can fail throws the press away in
 *  silence. Two such tests shipped and both were reported as a dead Send. Each
 *  asked whether the finger was still on the button at the lift. `TapPointer`
 *  above records why no coordinate answers that here, in any space.
 *
 *  So the lift half is given up, and `touchend` going to the element the press
 *  STARTED on is the guarantee left. A press sliding off a constructive button
 *  now fires it. That is the trade, and the reasoning is in
 *  `docs/plans/2026-08-26-a-tap-that-stays-on-the-button-sends.md`.
 *
 *  Local copies of this repair live in `promptFocus.ts` (`composeHandlers`,
 *  which adds focus-first) and `FileSearchModal.tsx`. */
export function touchActivated(action: () => void, opts: TouchActivateOptions = {}) {
  const enabled = opts.enabled ?? (() => true);
  const clock = opts.now ?? Date.now;
  let lastTouchAt: number | null = null;
  return {
    onTouchEnd(e: TouchEventLike): void {
      if (!enabled()) return;
      // Served, so the gate's press is spent rather than ruled on. See
      // `ActivationGate.spend`.
      opts.gate?.spend();
      lastTouchAt = clock();
      e.preventDefault();
      action();
    },
    onClick(): void {
      if (lastTouchAt !== null && clock() - lastTouchAt < TOUCH_CLICK_WINDOW_MS) {
        // A touch has ONE twin. Forget it here, so a genuine second tap landing
        // inside the window is served rather than eaten as well. Before the
        // gate, so a twin never rules on a press the touch path already spent.
        lastTouchAt = null;
        return;
      }
      // The gate is asked on this path alone. What it catches is the click iOS
      // fires after a touch that was starting a scroll. In front of BOTH paths
      // it would veto the touch path too, on the measurement named above.
      if (opts.gate && !opts.gate.pass()) return;
      action();
    },
  };
}
