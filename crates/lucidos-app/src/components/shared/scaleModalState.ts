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
 *  cancels it: Escape is the cancel for the zoom shortcuts, and the save is
 *  either releasing Cmd or letting the panel linger out. */
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

/** How long the panel lingers after the last zoom step, when a shortcut is what
 *  put it there. Releasing Cmd/Ctrl still dismisses it at once, but that keyup
 *  is not guaranteed to arrive, and it was the ONLY way out. The packaged macOS
 *  app is the reported case: the panel sat there for the rest of the session.
 *  Two more holes are structural rather than platform-specific. A chord
 *  forwarded out of an app iframe never produces a host keyup at all, and
 *  neither does tabbing away with the modifier still down. So the release is the
 *  fast path and this is the floor.
 *
 *  Every zoom step and wheel notch pushes the deadline out, so it measures
 *  idleness rather than total time on screen. Not a `--duration-*` token and
 *  deliberately unscaled by the animation-speed slider: this is a dwell period
 *  the user reads a number during, not a transition. */
export const SHORTCUT_LINGER_MS = 1500;

let lingerTimeout: ReturnType<typeof setTimeout> | undefined;

/** Start the countdown. Only a shortcut-opened panel gets one: the panel opened
 *  from Settings is a modal the user asked for, and has to sit there until they
 *  dismiss it. */
function armLinger() {
  clearTimeout(lingerTimeout);
  lingerTimeout = setTimeout(dismissScaleModal, SHORTCUT_LINGER_MS);
}

/** Push the deadline out for a panel already counting down, and do nothing for
 *  one that is not. That second half is what stops a zoom step inside the
 *  Settings modal starting a countdown on it. */
function renewLinger() {
  if (lingerTimeout !== undefined) armLinger();
}

function cancelLinger() {
  clearTimeout(lingerTimeout);
  lingerTimeout = undefined;
}

/** Test-only: disarm the pending save and the linger. Both are module state, so
 *  without this one case's armed timers fire during the next one. */
export function _resetScaleTimersForTesting(): void {
  cancelScheduledSave();
  cancelLinger();
}

export function openScaleModal() {
  cancelLinger();
  previewScale.value = currentUiScale();
  scaleModalOpen.value = true;
}

export function closeScaleModal() {
  // Escape and a click outside are a cancel, so a countdown left running would
  // re-persist the value this is about to revert away from.
  cancelLinger();
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
  cancelLinger();
  cancelScheduledSave();
  // Only when the engine has not already been told. The debounce is shorter than
  // the linger, so it usually wins the race. A Cmd held past it then lands here
  // with nothing left to say, and writing unconditionally spent a second
  // identical PUT on most zooms.
  if (previewScale.value !== currentUiScale()) void setUiScale(previewScale.value);
  scaleModalOpen.value = false;
}

function applyScaleChange(next: number) {
  const current = scaleModalOpen.value ? previewScale.value : currentUiScale();
  if (next === current) {
    // A step onto the value already shown is the user holding the shortcut down
    // against the clamp. Still them using it, so keep the panel up rather than
    // pulling it out from under them.
    renewLinger();
    return;
  }
  previewScale.value = next;
  applyUiScale(next);
  if (scaleModalOpen.value) renewLinger();
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
  // Settles the panel rather than renewing it. Renewing measures idleness, and
  // a thumb held still mid-drag is idle, so the panel would dissolve under the
  // pointer. Before the early return, so a drag pinned against either end of
  // the track settles it too.
  cancelLinger();
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
  // A `change` with no `input` before it (a tap straight onto the track) has to
  // settle the panel on its own.
  cancelLinger();
  const clamped = clampUiScale(val);
  // `change` can arrive for a value no `input` previewed (keyboard arrows on the
  // range input), so apply it here too rather than waiting out the debounce.
  if (clamped !== previewScale.value) {
    previewScale.value = clamped;
    applyUiScale(clamped);
  }
  scheduleSave(clamped, true);
}
