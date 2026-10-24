/** Is a primary pointer pressed RIGHT NOW?
 *
 *  `useDismissOnOutside` asks as an overlay opens. An overlay that opened
 *  UNDER a finger owes that gesture's trailing click to the gesture, never to
 *  itself. See `makeDismissHandlers`. `afterPressSettles` asks as focus leaves
 *  a field, so a relayout waits for the press that moved the focus.
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
  // A press held in a frame lifts there, so only its click reaches this document.
  document.addEventListener('click', (e) => { if (e.isTrusted) pressed = false; }, true);
  window.addEventListener('blur', () => { pressed = false; });
}

export function primaryPointerIsDown(): boolean {
  return pressed;
}

/** A press this module cannot see go down, or has already seen lift, still owes
 *  its click to the overlay opening now. A widget frame calls this for a finger
 *  held inside it. A long press calls it as its menu opens at the lift, which
 *  this module's capture listener has already cleared. */
export function notePressOwesClick(): void {
  pressed = true;
}

/** Run `fn` once the press now in progress has delivered its click.
 *
 *  A relayout on focus loss moves the button under the press. The release is
 *  hit-tested again, so its click goes to whatever moved there. A tap's compat
 *  mouse events and its click share one task, so the next task follows them.
 *  A mouse press first waits for its pointerup, which shares a task with its
 *  mouseup and click. A press that owes its click has already lifted, so it
 *  waits for that click. */
export function afterPressSettles(fn: () => void): void {
  if (!pressed) {
    setTimeout(fn, 0);
    return;
  }
  const ends = ['pointerup', 'pointercancel', 'click'] as const;
  const settle = (e: Event) => {
    if (e.type !== 'blur' && !isPrimaryPress(e)) return;
    for (const type of ends) document.removeEventListener(type, settle, true);
    window.removeEventListener('blur', settle);
    setTimeout(fn, 0);
  };
  for (const type of ends) document.addEventListener(type, settle, true);
  window.addEventListener('blur', settle);
}
