
let pressed = false;

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
  document.addEventListener('click', (e) => { if (e.isTrusted) pressed = false; }, true);
  window.addEventListener('blur', () => { pressed = false; });
}

export function primaryPointerIsDown(): boolean {
  return pressed;
}

export function notePressOwesClick(): void {
  pressed = true;
}

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
