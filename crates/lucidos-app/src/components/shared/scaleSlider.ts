import { UI_SCALE_MIN, UI_SCALE_MAX, UI_SCALE_STEP, clampUiScale } from '@lucidos/appearance';

export function fractionForScale(scale: number): number {
  return (clampUiScale(scale) - UI_SCALE_MIN) / (UI_SCALE_MAX - UI_SCALE_MIN);
}

export interface TrackMetrics {
  left: number;
  width: number;
  thumbWidth: number;
}

export function scaleAtPointer(clientX: number, track: TrackMetrics): number | null {
  const usable = track.width - track.thumbWidth;
  if (!(usable > 0)) return null;
  const raw = (clientX - track.left - track.thumbWidth / 2) / usable;
  const fraction = Math.min(1, Math.max(0, raw));
  return clampUiScale(UI_SCALE_MIN + fraction * (UI_SCALE_MAX - UI_SCALE_MIN));
}

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
