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

/** Every scale change is applied instantly and persisted on a trailing edge, so
 *  one continuous gesture costs one `PUT /preferences`. WebKit is generous with
 *  `change` on a range input, and the un-debounced commit this replaced fired a
 *  separate write per event: on an iOS PWA a single drag queued several
 *  concurrent PUTs, and one page suspend then failed all of them at once. */
const SAVE_DEBOUNCE_MS = 500;

let saveTimeout: ReturnType<typeof setTimeout> | undefined;

/** The scale an armed debounce would persist, and whether the user has
 *  COMMITTED to it. A commit is a released slider thumb: their decision, which a
 *  subsequent close must honour. A keyboard step is still tentative, so closing
 *  cancels it (Escape has always been the cancel for the zoom shortcuts, with
 *  releasing Cmd the save). */
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

/** Test-only: disarm any pending save. The debounce is module state, so without
 *  this one case's armed timer fires during the next one. */
export function _resetScheduledSaveForTesting(): void {
  cancelScheduledSave();
}

export function openScaleModal() {
  previewScale.value = currentUiScale();
  scaleModalOpen.value = true;
}

export function closeScaleModal() {
  scaleModalOpen.value = false;
  // Persist a committed value rather than discarding it: debouncing the slider
  // commit would otherwise turn "drag, then tap outside" into a silent revert.
  if (scheduled?.committed) {
    flushScheduledSave();
    return;
  }
  cancelScheduledSave();
  const saved = currentUiScale();
  if (previewScale.value !== saved) applyUiScale(saved);
}

export function dismissScaleModal() {
  cancelScheduledSave();
  void setUiScale(previewScale.value);
  scaleModalOpen.value = false;
}

function applyScaleChange(next: number) {
  const current = scaleModalOpen.value ? previewScale.value : currentUiScale();
  if (next === current) return;
  previewScale.value = next;
  applyUiScale(next);
  if (!scaleModalOpen.value) scaleModalOpen.value = true;
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
  const clamped = clampUiScale(val);
  if (clamped === previewScale.value) return;
  previewScale.value = clamped;
  applyUiScale(clamped);
  // Keep an armed save pointed at what the user can see: a debounce that fires
  // mid-drag must not persist a value they have already moved past.
  if (scheduled) scheduled = { ...scheduled, value: clamped };
}

// Does NOT close the modal — closing on slider mouse-up made it vanish
// the instant the user finished dragging on macOS Chrome.
export function commitSliderValue(val: number) {
  const clamped = clampUiScale(val);
  // `change` can arrive for a value no `input` previewed (keyboard arrows on the
  // range input), so apply it here too rather than waiting out the debounce.
  if (clamped !== previewScale.value) {
    previewScale.value = clamped;
    applyUiScale(clamped);
  }
  scheduleSave(clamped, true);
}
