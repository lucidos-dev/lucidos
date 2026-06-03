import { signal } from '@preact/signals';
import {
  applyUiScale,
  setUiScale,
  currentUiScale,
  clampUiScale,
  UI_SCALE_DEFAULT,
} from '../../store/actions/preferences';

export const scaleModalOpen = signal(false);
export const previewScale = signal(100);

let saveTimeout: ReturnType<typeof setTimeout> | undefined;

export function openScaleModal() {
  previewScale.value = currentUiScale();
  scaleModalOpen.value = true;
}

export function closeScaleModal() {
  clearTimeout(saveTimeout);
  scaleModalOpen.value = false;
  const saved = currentUiScale();
  if (previewScale.value !== saved) applyUiScale(saved);
}

export function dismissScaleModal() {
  clearTimeout(saveTimeout);
  void setUiScale(previewScale.value);
  scaleModalOpen.value = false;
}

function applyScaleChange(next: number) {
  const current = scaleModalOpen.value ? previewScale.value : currentUiScale();
  if (next === current) return;
  previewScale.value = next;
  applyUiScale(next);
  if (!scaleModalOpen.value) scaleModalOpen.value = true;
  clearTimeout(saveTimeout);
  saveTimeout = setTimeout(() => void setUiScale(next), 500);
}

export function resetUiScale() {
  applyScaleChange(UI_SCALE_DEFAULT);
}

export function adjustUiScale(delta: number) {
  const base = scaleModalOpen.value ? previewScale.value : currentUiScale();
  applyScaleChange(clampUiScale(base + delta));
}

export function previewSliderValue(val: number) {
  const clamped = clampUiScale(val);
  if (clamped === previewScale.value) return;
  previewScale.value = clamped;
  applyUiScale(clamped);
}

// Does NOT close the modal — closing on slider mouse-up made it vanish
// the instant the user finished dragging on macOS Chrome.
export function commitSliderValue(val: number) {
  const clamped = clampUiScale(val);
  clearTimeout(saveTimeout);
  previewScale.value = clamped;
  void setUiScale(clamped);
}
