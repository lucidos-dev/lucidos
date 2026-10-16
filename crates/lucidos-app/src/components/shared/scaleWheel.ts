/**
 * How far a wheel gesture has asked the UI scale to move, and when that is
 * enough to be worth a step.
 *
 * The panel used to take every ctrl/meta wheel event as one full
 * `UI_SCALE_STEP`. That reads a MOUSE correctly, where one physical notch is one
 * event. It reads a trackpad pinch as dozens of steps, because a pinch is a
 * stream of small-delta events rather than a count of notches. So one gesture
 * slammed the scale to its clamp, and the work it queued is what wedged the tab
 * (`docs/plans/2026-09-19-zooming-cannot-wedge-the-tab.md`).
 *
 * Distance is the honest unit for both devices. A notch is worth one step
 * whatever the device reports it as, and a pinch spends the same currency.
 *
 * Pure, so the decision is testable without a wheel.
 */

/** Pixel-equivalent distance of one classic wheel notch, and therefore the
 *  natural size of one scale step. Chrome reports it for one detent on Windows.
 *  macOS reports a slow tick as far less, which `foldWheelEvent` covers. */
const PX_PER_NOTCH = 100;

/** Lines in one notch, for the engines that report `DOM_DELTA_LINE` (Firefox
 *  with a mouse). Converting through the notch rather than through a line box
 *  keeps a physical detent worth exactly one step on every engine. */
const LINES_PER_NOTCH = 3;

/** Notches in one `DOM_DELTA_PAGE` unit. No engine in our support set reports
 *  page mode, so this exists to keep the conversion total rather than to be
 *  exact. */
const NOTCHES_PER_PAGE = 8;

/** Quiet time that ENDS a gesture. Banked distance does not survive it, so a
 *  slow scroll cannot bank a step over a minute and spend it later. Comfortably
 *  longer than the gap between two events of one pinch. */
export const WHEEL_GESTURE_GAP_MS = 150;

/** What one gesture has asked for and not yet been given. */
export interface WheelBank {
  /** Unspent distance, in pixel-equivalents. Positive means zoom IN. */
  distance: number;
  /** `nowMs` of the last event folded in. */
  lastEventAt: number;
}

export function emptyWheelBank(): WheelBank {
  return { distance: 0, lastEventAt: 0 };
}

/** One wheel event as a signed zoom distance.
 *
 *  The sign is flipped from the event's: `deltaY` runs positive downward, and
 *  scrolling down zooms out. */
export function wheelZoomDistance(deltaY: number, deltaMode: number): number {
  if (!Number.isFinite(deltaY)) return 0;
  switch (deltaMode) {
    case 1: return -deltaY * (PX_PER_NOTCH / LINES_PER_NOTCH);
    case 2: return -deltaY * PX_PER_NOTCH * NOTCHES_PER_PAGE;
    default: return -deltaY;
  }
}

/** Fold one event's distance into the bank.
 *
 *  Two things empty it first. An idle gap means the previous gesture ended, and
 *  a reversal means the user changed their mind. Carrying either would make the
 *  new direction wait out distance the old one banked.
 *
 *  A wheel turned with Cmd or Ctrl held opens its gesture with at least one
 *  notch. macOS reports a slow mouse tick as a few pixels. Each tick also
 *  arrives after the gap, so without the floor a slow wheel never steps. A pinch
 *  gets no floor: Chrome sends it as ctrl-wheel with no key down, and its first
 *  event is a sliver of the gesture, not a notch. A zero-distance event folds
 *  nothing in, so it cannot open a gesture and spend the floor. */
export function foldWheelEvent(
  bank: WheelBank, distance: number, nowMs: number, modifierKeyHeld: boolean,
): WheelBank {
  if (distance === 0) return bank;
  const stale = nowMs - bank.lastEventAt > WHEEL_GESTURE_GAP_MS;
  const reversed = bank.distance !== 0
    && Math.sign(distance) !== Math.sign(bank.distance);
  if (!(stale || reversed)) {
    return { distance: bank.distance + distance, lastEventAt: nowMs };
  }
  const floor = modifierKeyHeld ? PX_PER_NOTCH : 0;
  const opening = Math.sign(distance) * Math.max(Math.abs(distance), floor);
  return { distance: opening, lastEventAt: nowMs };
}

/** Spend one step's worth of banked distance, or answer 0 when there is not
 *  enough. The remainder stays banked, so a gesture spends exactly what it
 *  travelled and a caller can drain one step per frame. */
export function takeWheelStep(bank: WheelBank): { bank: WheelBank; step: number } {
  if (Math.abs(bank.distance) < PX_PER_NOTCH) return { bank, step: 0 };
  const step = Math.sign(bank.distance);
  return {
    bank: { ...bank, distance: bank.distance - step * PX_PER_NOTCH },
    step,
  };
}

/** A live wheel-zoom gesture: events in, at most one step per frame out.
 *
 *  The frame cap is the hard bound, and it holds whatever the accumulator says.
 *  No input device can ask for more scale changes than the renderer can paint,
 *  so a sustained gesture can only jank.
 *
 *  Here rather than in the listener, so the whole policy is one testable unit
 *  and `scaleWheelListener.ts` only decides which events are its own. */
export function createWheelZoom(step: (direction: number) => void) {
  let bank = emptyWheelBank();
  let frame: number | null = null;

  function drain(): void {
    frame = null;
    const taken = takeWheelStep(bank);
    bank = taken.bank;
    if (taken.step === 0) return;
    step(taken.step);
    // Whatever is left over is the NEXT frame's.
    schedule();
  }

  function schedule(): void {
    if (frame !== null) return;
    frame = requestAnimationFrame(drain);
  }

  return {
    /** Fold one event in. The caller has already decided it is a zoom. */
    push(deltaY: number, deltaMode: number, at: number, modifierKeyHeld: boolean): void {
      bank = foldWheelEvent(bank, wheelZoomDistance(deltaY, deltaMode), at, modifierKeyHeld);
      schedule();
    },
    /** Drop the pending frame and whatever distance is still banked, so a
     *  gesture the panel no longer serves cannot step after it closes. */
    stop(): void {
      bank = emptyWheelBank();
      if (frame === null) return;
      cancelAnimationFrame(frame);
      frame = null;
    },
  };
}
