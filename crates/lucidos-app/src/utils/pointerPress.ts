/** Is a primary pointer pressed RIGHT NOW?
 *
 *  One caller, one moment: `useDismissOnOutside` asks as an overlay opens. An
 *  overlay that opened UNDER a finger owes that gesture's trailing click to the
 *  gesture, never to itself. See `makeDismissHandlers`.
 *
 *  TRUSTED events only, because the real question is whether the BROWSER will
 *  pair a click with this press. It pairs one only with a real gesture. Count a
 *  dispatched `PointerEvent` and the next synthetic click is read as that
 *  pairing, which two e2e specs on the same drawer menu would catch.
 *
 *  PRIMARY pointers only, so a second finger's lift cannot clear a press the
 *  first finger still holds.
 *
 *  Installed at import, since the answer is about a press that has ALREADY
 *  started. A window BLUR clears it too: a release delivered elsewhere is a
 *  press this document never sees end. A stranded `true` then costs the next
 *  overlay one synthetic dismiss.
 */

let pressed = false;

/** Every listener below asks the same two questions of its event. */
function isPrimaryPress(e: Event): boolean {
  return e.isTrusted && (e as PointerEvent).isPrimary !== false;
}

if (typeof document !== 'undefined') {
  document.addEventListener('pointerdown', (e) => {
    if (isPrimaryPress(e) && (e as PointerEvent).button === 0) pressed = true;
  }, true);
  const release = (e: Event) => { if (isPrimaryPress(e)) pressed = false; };
  document.addEventListener('pointerup', release, true);
  document.addEventListener('pointercancel', release, true);
  window.addEventListener('blur', () => { pressed = false; });
}

export function primaryPointerIsDown(): boolean {
  return pressed;
}
