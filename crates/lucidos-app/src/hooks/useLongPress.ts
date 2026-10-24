import { hapticTick } from '@lucidos/haptic';
import { LONG_PRESS_DELAY_MS, PRESS_MOVE_TOLERANCE_PX } from '@lucidos/widget-frame';
import { useEffect, useRef } from 'preact/hooks';
import { notePressOwesClick } from '../utils/pointerPress';
import type { ViewportPoint } from './useAnchoredPopover';

export interface LongPressHandlers {
  onPointerDown(e: PointerEvent): void;
  onPointerMove(e: PointerEvent): void;
  onPointerUp(e: PointerEvent): void;
  onPointerLeave(e: PointerEvent): void;
  onPointerCancel(e: PointerEvent): void;
  onContextMenu(e: MouseEvent): void;
  onClick(e: MouseEvent): void;
  /** Drop any pending press and suppress-fuse. Call on unmount so a late lift
   *  or timer can't fire into a torn-down component. */
  cancel(): void;
}

export interface LongPressOptions {
  /** Hold duration before the long-press fires. */
  delayMs?: number;
  /** Pointer travel (px) that cancels a pending hold: a drag, not a press. */
  moveTolerancePx?: number;
  /** Fire once the finger has been still for `delayMs`, before it lifts. Only
   *  for a hold that starts a drag, which must begin under the finger. A lost
   *  lift then fires it too, which is why nothing else opts in. */
  firesWhileHeld?: boolean;
}

export { LONG_PRESS_DELAY_MS };
const DEFAULT_MOVE_TOLERANCE_PX = PRESS_MOVE_TOLERANCE_PX;
// Upper bound for swallowing the `click` the browser pairs with the gesture's
// pointerup. Keyed to pointerup (not to fire time) so an arbitrarily long hold
// can never leave the suppressor armed against a later, unrelated click.
const SUPPRESS_FUSE_MS = 700;
const NATIVE_LONG_PRESS_PROPERTIES = ['user-select', '-webkit-user-select', '-webkit-touch-callout'];

/** Turn off iOS's own answer to a resting finger on `target`, until the next
 *  press anywhere. iOS shows its selection or link menu before the lift opens
 *  ours, and a selection also cancels the gesture. Returns the undo. */
function holdNativeLongPress(target: HTMLElement): () => void {
  const { style } = target;
  const before = NATIVE_LONG_PRESS_PROPERTIES.map((property) =>
    [property, style.getPropertyValue(property), style.getPropertyPriority(property)] as const);
  for (const property of NATIVE_LONG_PRESS_PROPERTIES) style.setProperty(property, 'none', 'important');
  const release = () => {
    document.removeEventListener('pointerdown', release, true);
    for (const [property, value, priority] of before) {
      if (value) style.setProperty(property, value, priority);
      else style.removeProperty(property);
    }
  };
  document.addEventListener('pointerdown', release, true);
  return release;
}

/** Where a right-click landed. A hold passes none: the finger covers the
 *  point, so its host places the menu against the target instead. */
export type LongPressCallback = (target: HTMLElement, at?: ViewportPoint) => void;

/** Pure long-press / right-click gesture machine. Returns DOM handlers that
 *  call `onLongPress(target)`, with a haptic tick, when a still pointer lifts
 *  at least `delayMs` after its press. A right-click `contextmenu` calls `onLongPress(target, at)`,
 *  and an ordinary tap calls `onClick(e)`.
 *
 *  A hold fires at its LIFT, never on a timer. iOS can take a gesture and send
 *  the page no lift at all, so silence cannot tell a held finger from a lost
 *  one. A timer read that silence as a hold, and opened a side question under a
 *  tap on Send. The lift is read at `document` capture, ahead of the pressed
 *  button's own `touchend` and `pointerup`, so the release never also acts.
 *  A touch press turns off the platform's own long-press answer at once,
 *  since iOS would show it before the lift.
 *  The `click` the browser pairs with a fired long-press is swallowed
 *  (`preventDefault` + `stopPropagation`) so the host button's primary action
 *  doesn't also run.
 *
 *  Exported as a hook-free factory so it's unit-testable with fake timers;
 *  `useLongPress` wires it to a component with stable refs. */
export function makeLongPressHandlers(
  onLongPress: LongPressCallback,
  onClick: (e: MouseEvent) => void,
  opts: LongPressOptions = {},
): LongPressHandlers {
  const delayMs = opts.delayMs ?? LONG_PRESS_DELAY_MS;
  const moveTolerance = opts.moveTolerancePx ?? DEFAULT_MOVE_TOLERANCE_PX;

  const firesWhileHeld = opts.firesWhileHeld ?? false;

  /** The press that may still become a hold, or null once it cannot. */
  let pending: { target: HTMLElement; downAt: number } | null = null;
  let timer: ReturnType<typeof setTimeout> | null = null;
  let fuse: ReturnType<typeof setTimeout> | null = null;
  let startX = 0;
  let startY = 0;
  let armed = false; // a long-press fired; swallow the paired click
  let unwatchLift: (() => void) | null = null;
  let releaseNativeLongPress: (() => void) | null = null;

  function endPress() {
    pending = null;
    if (timer !== null) { clearTimeout(timer); timer = null; }
    unwatchLift?.();
    unwatchLift = null;
  }
  function fire(target: HTMLElement) {
    endPress();
    armed = true;
    hapticTick();
    onLongPress(target);
  }
  /** Rule on the pending press at its lift, from the lift's own time. */
  function lift(e: Event) {
    const press = pending;
    if (press && e.timeStamp - press.downAt >= delayMs) {
      // The menu opens at the lift, which `pointerPress` already cleared. The
      // lift's trailing click belongs to the hold, never to the new menu. Only
      // a trusted lift gets a click from the browser.
      if (e.isTrusted) notePressOwesClick();
      fire(press.target);
    } else endPress();
  }
  /** Hear this press's lift anywhere: its pointer lifting, or the last finger
   *  leaving the glass. A tap's action can swap the button out, or disable it
   *  on `touchend`. WebKit then dispatches no pointerup at all. A cancel is the
   *  system taking the gesture, which is never a hold. A later press ends a
   *  press whose lift was lost, so the next tap's lift never rules on it. */
  function watchLift(pointerId: number) {
    const nextPress = () => endPress();
    const pointerLift = (e: PointerEvent) => { if (e.pointerId === pointerId) lift(e); };
    const pointerCancel = (e: PointerEvent) => { if (e.pointerId === pointerId) endPress(); };
    const touchLift = (e: TouchEvent) => { if (e.touches.length === 0) lift(e); };
    const touchCancel = (e: TouchEvent) => { if (e.touches.length === 0) endPress(); };
    document.addEventListener('pointerdown', nextPress, true);
    document.addEventListener('pointerup', pointerLift, true);
    document.addEventListener('pointercancel', pointerCancel, true);
    document.addEventListener('touchend', touchLift, true);
    document.addEventListener('touchcancel', touchCancel, true);
    unwatchLift = () => {
      document.removeEventListener('pointerdown', nextPress, true);
      document.removeEventListener('pointerup', pointerLift, true);
      document.removeEventListener('pointercancel', pointerCancel, true);
      document.removeEventListener('touchend', touchLift, true);
      document.removeEventListener('touchcancel', touchCancel, true);
    };
  }
  function disarm() {
    armed = false;
    if (fuse !== null) { clearTimeout(fuse); fuse = null; }
  }
  function armFuse() {
    if (fuse !== null) clearTimeout(fuse);
    fuse = setTimeout(() => { fuse = null; armed = false; }, SUPPRESS_FUSE_MS);
  }

  return {
    onPointerDown(e) {
      // Only the primary button arms a hold; right-click flows through
      // onContextMenu so the two triggers never double-fire. A fresh press
      // also clears any stranded arm from a prior gesture that emitted no
      // paired click (e.g. a right-click).
      if (e.button !== 0) return;
      disarm();
      endPress();
      startX = e.clientX;
      startY = e.clientY;
      const target = e.currentTarget as HTMLElement;
      pending = { target, downAt: e.timeStamp };
      // No `setPointerCapture`: it retargets the paired `click`, and it
      // suppresses the boundary events `onPointerLeave` cancels on
      // (`docs/known-gaps.md`).
      if (firesWhileHeld) timer = setTimeout(() => fire(target), delayMs);
      if (e.pointerType !== 'mouse') releaseNativeLongPress = holdNativeLongPress(target);
      watchLift(e.pointerId);
    },
    onPointerMove(e) {
      if (pending === null) return;
      if (Math.abs(e.clientX - startX) > moveTolerance ||
          Math.abs(e.clientY - startY) > moveTolerance) {
        endPress();
      }
    },
    onPointerUp(e) {
      // The document listener has usually ruled already. A caller that
      // forwards a lift it heard elsewhere is ruled here.
      if (pending !== null) lift(e);
      // If a long-press fired, the paired click is imminent: keep the arm
      // alive just long enough to swallow it, then auto-disarm.
      if (armed && fuse === null) armFuse();
    },
    onPointerLeave() {
      endPress();
    },
    onPointerCancel() {
      endPress();
      disarm();
    },
    onContextMenu(e) {
      e.preventDefault();
      endPress();
      // A mouse right-click dispatches no `click`, so the arm is harmless there;
      // a touch long-press that surfaces as `contextmenu` may pair one, which
      // the fused arm then swallows.
      armed = true;
      armFuse();
      onLongPress(e.currentTarget as HTMLElement, { x: e.clientX, y: e.clientY });
    },
    onClick(e) {
      if (armed) {
        disarm();
        e.preventDefault();
        e.stopPropagation();
        return;
      }
      onClick(e);
    },
    cancel() {
      endPress();
      disarm();
      releaseNativeLongPress?.();
      releaseNativeLongPress = null;
    },
  };
}

/** Component-friendly wrapper over {@link makeLongPressHandlers}. The gesture
 *  state lives in one ref so it survives re-renders; the callbacks are read
 *  through refs so the handlers always invoke the latest closures without being
 *  rebuilt (which would reset the in-flight gesture). */
export function useLongPress(
  onLongPress: LongPressCallback,
  onClick: (e: MouseEvent) => void,
  opts?: LongPressOptions,
): LongPressHandlers {
  const longPressRef = useRef(onLongPress);
  longPressRef.current = onLongPress;
  const clickRef = useRef(onClick);
  clickRef.current = onClick;
  const handlersRef = useRef<LongPressHandlers>();
  if (!handlersRef.current) {
    handlersRef.current = makeLongPressHandlers(
      (t, at) => longPressRef.current(t, at),
      (e) => clickRef.current(e),
      opts,
    );
  }
  // Drop any pending press / fuse if the host unmounts mid-gesture (e.g. a
  // viewport breakpoint swaps the header layout while the button is held).
  useEffect(() => () => handlersRef.current?.cancel(), []);
  return handlersRef.current;
}
