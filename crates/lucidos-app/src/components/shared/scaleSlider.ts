/**
 * Where the UI-scale slider's thumb sits, and which scale a pointer or a key is
 * asking for. Split out of the component so all three are testable without a
 * layout engine.
 *
 * The slider is pointer-driven rather than an `<input type="range">`. WebKit
 * starts a range drag only on the thumb, so a touch beside it did nothing. That
 * is why the thumb had grown to 40px. Owning the input makes the whole row the
 * hit target, which frees the thumb to be a normal size again.
 *
 * The first two live together on purpose. CSS places the thumb from
 * `fractionForScale`, and a touch reads back through `scaleAtPointer`. Let the
 * two disagree and the thumb slides out from under the finger.
 */
import { UI_SCALE_MIN, UI_SCALE_MAX, UI_SCALE_STEP, clampUiScale } from '@lucidos/appearance';

/** The thumb's centre as a 0..1 fraction of the span it can travel. */
export function fractionForScale(scale: number): number {
  return (clampUiScale(scale) - UI_SCALE_MIN) / (UI_SCALE_MAX - UI_SCALE_MIN);
}

/** The slider row, measured once at the start of a gesture. */
export interface TrackMetrics {
  /** The row's left edge, in client coordinates. */
  left: number;
  /** The row's full width, thumb included. */
  width: number;
  /** The thumb's width. Half of it is unusable track at each end. */
  thumbWidth: number;
}

/**
 * The scale a pointer at `clientX` is asking for, snapped to the 12.5 grid.
 *
 * The thumb's centre travels between `thumbWidth / 2` and
 * `width - thumbWidth / 2`, or it would hang off the ends of the track. The
 * pointer maps onto that inset span. So the finger stays on the thumb at both
 * extremes, instead of drifting half a thumb away from it.
 *
 * `null` when the row has no usable width yet, which the caller reads as
 * "nothing to apply".
 */
export function scaleAtPointer(clientX: number, track: TrackMetrics): number | null {
  const usable = track.width - track.thumbWidth;
  if (!(usable > 0)) return null;
  const raw = (clientX - track.left - track.thumbWidth / 2) / usable;
  const fraction = Math.min(1, Math.max(0, raw));
  return clampUiScale(UI_SCALE_MIN + fraction * (UI_SCALE_MAX - UI_SCALE_MIN));
}

/**
 * The scale a key asks for, or `null` when the key is not one of the slider's.
 *
 * A `role="slider"` that answers only a pointer is a lie, and the range input
 * this replaced took these keys once it was focused. `null` is what tells the
 * handler to leave the event alone, so every other key still bubbles.
 */
export function scaleAfterKey(key: string, current: number): number | null {
  switch (key) {
    case 'ArrowRight':
    case 'ArrowUp':
      return clampUiScale(current + UI_SCALE_STEP);
    case 'ArrowLeft':
    case 'ArrowDown':
      return clampUiScale(current - UI_SCALE_STEP);
    case 'Home':
      return UI_SCALE_MIN;
    case 'End':
      return UI_SCALE_MAX;
    default:
      return null;
  }
}
