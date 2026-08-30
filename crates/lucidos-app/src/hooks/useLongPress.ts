import { useEffect, useRef } from 'preact/hooks';

export interface LongPressHandlers {
  onPointerDown(e: PointerEvent): void;
  onPointerMove(e: PointerEvent): void;
  onPointerUp(e: PointerEvent): void;
  onPointerLeave(e: PointerEvent): void;
  onPointerCancel(e: PointerEvent): void;
  onContextMenu(e: MouseEvent): void;
  onClick(e: MouseEvent): void;
  /** Clear any pending hold-timer / suppress-fuse. Call on unmount so a timer
   *  can't fire into a torn-down component. */
  cancel(): void;
}

export interface LongPressOptions {
  /** Hold duration before the long-press fires. */
  delayMs?: number;
  /** Pointer travel (px) that cancels a pending hold — a drag, not a press. */
  moveTolerancePx?: number;
}

const DEFAULT_DELAY_MS = 450;
const DEFAULT_MOVE_TOLERANCE_PX = 10;
// Upper bound for swallowing the `click` the browser pairs with the gesture's
// pointerup. Keyed to pointerup (not to fire time) so an arbitrarily long hold
// can never leave the suppressor armed against a later, unrelated click.
const SUPPRESS_FUSE_MS = 700;

/** Pure long-press / right-click gesture machine. Returns DOM handlers that
 *  call `onLongPress(target)` when the pointer is held still past `delayMs`
 *  (or on a right-click `contextmenu`), and `onClick()` for an ordinary tap.
 *  The `click` the browser pairs with a fired long-press is swallowed
 *  (`preventDefault` + `stopPropagation`) so the host button's primary action
 *  doesn't also run.
 *
 *  Exported as a hook-free factory so it's unit-testable with fake timers;
 *  `useLongPress` wires it to a component with stable refs. */
export function makeLongPressHandlers(
  onLongPress: (target: HTMLElement) => void,
  onClick: () => void,
  opts: LongPressOptions = {},
): LongPressHandlers {
  const delayMs = opts.delayMs ?? DEFAULT_DELAY_MS;
  const moveTolerance = opts.moveTolerancePx ?? DEFAULT_MOVE_TOLERANCE_PX;

  let timer: ReturnType<typeof setTimeout> | null = null;
  let fuse: ReturnType<typeof setTimeout> | null = null;
  let startX = 0;
  let startY = 0;
  let armed = false; // a long-press fired; swallow the paired click

  function clearTimer() {
    if (timer !== null) { clearTimeout(timer); timer = null; }
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
      clearTimer();
      startX = e.clientX;
      startY = e.clientY;
      const target = e.currentTarget as HTMLElement;
      // No `setPointerCapture`: it retargets the paired `click`, and it
      // suppresses the boundary events `onPointerLeave` cancels on
      // (`docs/known-gaps.md`).
      timer = setTimeout(() => {
        timer = null;
        armed = true;
        onLongPress(target);
      }, delayMs);
    },
    onPointerMove(e) {
      if (timer === null) return;
      if (Math.abs(e.clientX - startX) > moveTolerance ||
          Math.abs(e.clientY - startY) > moveTolerance) {
        clearTimer();
      }
    },
    onPointerUp() {
      clearTimer();
      // If a long-press fired, the paired click is imminent: keep the arm
      // alive just long enough to swallow it, then auto-disarm.
      if (armed && fuse === null) armFuse();
    },
    onPointerLeave() {
      clearTimer();
    },
    onPointerCancel() {
      clearTimer();
      disarm();
    },
    onContextMenu(e) {
      e.preventDefault();
      clearTimer();
      // A mouse right-click dispatches no `click`, so the arm is harmless there;
      // a touch long-press that surfaces as `contextmenu` may pair one, which
      // the fused arm then swallows.
      armed = true;
      armFuse();
      onLongPress(e.currentTarget as HTMLElement);
    },
    onClick(e) {
      if (armed) {
        disarm();
        e.preventDefault();
        e.stopPropagation();
        return;
      }
      onClick();
    },
    cancel() {
      clearTimer();
      disarm();
    },
  };
}

/** Component-friendly wrapper over {@link makeLongPressHandlers}. The gesture
 *  state lives in one ref so it survives re-renders; the callbacks are read
 *  through refs so the handlers always invoke the latest closures without being
 *  rebuilt (which would reset the in-flight gesture). */
export function useLongPress(
  onLongPress: (target: HTMLElement) => void,
  onClick: () => void,
  opts?: LongPressOptions,
): LongPressHandlers {
  const longPressRef = useRef(onLongPress);
  longPressRef.current = onLongPress;
  const clickRef = useRef(onClick);
  clickRef.current = onClick;
  const handlersRef = useRef<LongPressHandlers>();
  if (!handlersRef.current) {
    handlersRef.current = makeLongPressHandlers(
      (t) => longPressRef.current(t),
      () => clickRef.current(),
      opts,
    );
  }
  // Clear any pending hold-timer / fuse if the host unmounts mid-gesture (e.g. a
  // viewport breakpoint swaps the header layout while the button is held).
  useEffect(() => () => handlersRef.current?.cancel(), []);
  return handlersRef.current;
}
