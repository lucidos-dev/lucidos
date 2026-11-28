import { signal } from '@preact/signals';
import {
  previewUiScale,
  cancelUiScalePreview,
  setUiScale,
  currentUiScale,
  clampUiScale,
  UI_SCALE_DEFAULT,
} from '../../store/actions/preferences';

export const scaleModalOpen = signal(false);
export const previewScale = signal(100);

const SAVE_DEBOUNCE_MS = 500;

let saveTimeout: ReturnType<typeof setTimeout> | undefined;

let scheduled: { value: number; committed: boolean } | null = null;

function scheduleSave(value: number, committed: boolean) {
  clearTimeout(saveTimeout);
  scheduled = { value, committed };
  saveTimeout = setTimeout(flushScheduledSave, SAVE_DEBOUNCE_MS);
}

function flushScheduledSave() {
  clearTimeout(saveTimeout);
  const pending = scheduled;
  scheduled = null;
  if (pending) void setUiScale(pending.value);
}

function cancelScheduledSave() {
  clearTimeout(saveTimeout);
  scheduled = null;
}

export const SHORTCUT_LINGER_MS = 1500;

let lingerTimeout: ReturnType<typeof setTimeout> | undefined;

function armLinger() {
  clearTimeout(lingerTimeout);
  lingerTimeout = setTimeout(dismissScaleModal, SHORTCUT_LINGER_MS);
}

export function renewScaleModalLinger() {
  if (lingerTimeout !== undefined) armLinger();
}

function cancelLinger() {
  clearTimeout(lingerTimeout);
  lingerTimeout = undefined;
}

export function _resetScaleTimersForTesting(): void {
  cancelScheduledSave();
  cancelLinger();
  cancelUiScalePreview();
}

export function openScaleModal() {
  cancelLinger();
  previewScale.value = currentUiScale();
  scaleModalOpen.value = true;
}

export function closeScaleModal() {
  cancelLinger();
  scaleModalOpen.value = false;
  if (scheduled?.committed) {
    flushScheduledSave();
    return;
  }
  cancelScheduledSave();
  cancelUiScalePreview();
}

export function dismissScaleModal() {
  cancelLinger();
  cancelScheduledSave();
  if (previewScale.value !== currentUiScale()) void setUiScale(previewScale.value);
  else cancelUiScalePreview();
  scaleModalOpen.value = false;
}

function applyScaleChange(next: number) {
  const current = scaleModalOpen.value ? previewScale.value : currentUiScale();
  if (next === current) {
    renewScaleModalLinger();
    return;
  }
  previewScale.value = next;
  previewUiScale(next);
  if (scaleModalOpen.value) renewScaleModalLinger();
  else {
    scaleModalOpen.value = true;
    armLinger();
  }
  scheduleSave(next, false);
}

export function resetUiScale() {
  applyScaleChange(UI_SCALE_DEFAULT);
}

export function adjustUiScale(delta: number) {
  const base = scaleModalOpen.value ? previewScale.value : currentUiScale();
  applyScaleChange(clampUiScale(base + delta));
}

export function previewSliderValue(val: number) {
  cancelLinger();
  const clamped = clampUiScale(val);
  if (clamped === previewScale.value) return;
  previewScale.value = clamped;
  previewUiScale(clamped);
  if (scheduled) scheduled = { ...scheduled, value: clamped };
}

export function commitSliderValue(val: number) {
  cancelLinger();
  const clamped = clampUiScale(val);
  if (clamped !== previewScale.value) {
    previewScale.value = clamped;
    previewUiScale(clamped);
  }
  scheduleSave(clamped, true);
}
