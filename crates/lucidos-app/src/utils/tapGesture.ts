// Movement during a press that is treated as a scroll instead of a tap.
// 8 px matches the swipe direction-lock threshold in `swipe.ts`.
// Exported for `deadPressProbe`, which asks the same question of a cancelled
// gesture: did the finger move, or did the system take a stationary press?
export const TAP_MOVE_THRESHOLD_PX = 8;

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
  /** The SYSTEM took the gesture, so nothing may activate on it.
   *
   *  Held apart from `state` because clearing the press cannot say this. A
   *  keyboard Enter has no press either and must still activate, so "the gate
   *  holds nothing" has to keep meaning yes. Cleared by the next `down`. */
  let aborted = false;

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
      aborted = false;
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
    /** `onPointerCancel`: the system took the gesture for a scroll or a zoom.
     *  The lift may still arrive, and nothing may activate on it. */
    cancel(): void {
      state = null;
      aborted = true;
    },
    /** A path consumed the press. Forget it cleanly, leaving the gesture's
     *  standing alone: this is the user's action running, not a scroll. */
    spend(): void {
      state = null;
    },
    /** Whether the system took the in-flight gesture. Asked by a path that
     *  runs INSIDE the gesture, since the browser cannot withhold the lift the
     *  way it withholds the click. */
    wasAborted(): boolean {
      return aborted;
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

/** Who took a touch press, said by whoever took it.
 *
 *  `served` is a button running its action inside the gesture. `swallowed` is
 *  the overlay contract eating the paired event of a dismissing tap.
 *
 *  The two are indistinguishable to an observer, which is the whole reason this
 *  exists. Both cancel the default, and the swallow also stops propagation at
 *  `document` in the capture phase. `deadPressProbe` read `defaultPrevented` as
 *  proof a path had worked and was wrong about both. It now asks here. */
export type PressOutcome = 'served' | 'swallowed';

let lastPress: { outcome: PressOutcome; at: number } | null = null;

/** Record who took the press currently being dispatched. */
export function notePressOutcome(outcome: PressOutcome, now: number = Date.now()): void {
  lastPress = { outcome, at: now };
}

/** The outcome recorded within `withinMs`, or null when nobody claimed the
 *  press. Consuming, so one press's outcome can never describe the next. */
export function takePressOutcome(withinMs: number, now: number = Date.now()): PressOutcome | null {
  const press = lastPress;
  lastPress = null;
  if (press === null || now - press.at > withinMs) return null;
  return press.outcome;
}

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
  /** Did the system take this gesture? The click path never has to ask: a
   *  browser that cancels a pointer sends no click either. A path running
   *  inside the gesture does, because the lift arrives regardless. */
  aborted?(): boolean;
}

export interface TouchActivateOptions {
  /** Stands the TOUCH path down while it returns false, leaving the click path
   *  alone. A button whose action turns destructive in some state passes that
   *  state here, so the state keeps ordinary click activation. */
  enabled?: () => boolean;
  /** The scroll-vs-tap gate. See `onClick` for why the touch path spends the
   *  press rather than asking. Absent means every click activates. */
  gate?: ActivationGate;
  /** A face whose action cannot be taken back. It RULES on the gate instead of
   *  spending it, on the touch path as well as the click path.
   *
   *  The touch path otherwise takes every press it is given, which is right for
   *  Send and wrong for Cancel: a scroll landing there would abort a live turn.
   *  Withholding the touch path altogether was the earlier answer, and it left
   *  Cancel with no path at all once iOS dropped the click.
   *
   *  A thunk, because one node serves Send and Cancel. Read at the lift. */
  destructive?: () => boolean;
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
 *  **The CONSTRUCTIVE touch path takes every press.** With a field focused on iOS
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
 *  A DESTRUCTIVE face is the exception and asks the gate, so a scroll cannot
 *  fire it. See `opts.destructive`.
 *
 *  Local copies of this repair live in `promptFocus.ts` (`composeHandlers`,
 *  which adds focus-first) and `FileSearchModal.tsx`. */
export function touchActivated(action: () => void, opts: TouchActivateOptions = {}) {
  const enabled = opts.enabled ?? (() => true);
  const destructive = opts.destructive ?? (() => false);
  const clock = opts.now ?? Date.now;
  let lastTouchAt: number | null = null;
  return {
    onTouchEnd(e: TouchEventLike): void {
      if (!enabled()) return;
      // Cancel and record the twin BEFORE the gate rules, so a refused press
      // cannot return as a click. `pass()` consumes the press, so that click
      // would arrive with none behind it and run what was just refused.
      lastTouchAt = clock();
      e.preventDefault();
      // A destructive face asks; every other face serves the press and spends
      // the gate's copy of it. See `ActivationGate` and `opts.destructive`.
      if (destructive()) {
        // A scroll makes the browser take the pointer, and the lift still
        // arrives. The gate holds no press by then, which reads as a tap, so
        // the abort has to be asked about separately.
        if (opts.gate?.aborted?.()) return;
        if (!(opts.gate?.pass() ?? true)) return;
      } else {
        opts.gate?.spend();
      }
      // Before the action, not after: a throwing action still means a path
      // took this press, and the probe must not call it dead.
      notePressOutcome('served');
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
      // What the gate catches here is the click iOS fires after a touch that
      // was starting a scroll. Unconditionally in front of BOTH paths it would
      // veto a constructive touch too, on the measurement named above.
      if (opts.gate && !opts.gate.pass()) return;
      action();
    },
  };
}
