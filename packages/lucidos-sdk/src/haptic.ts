/**
 * One light haptic tick, for a hold that opened a menu. A native long-press
 * menu ticks, so ours must too, or the press feels dead.
 *
 * Android has `navigator.vibrate`. iOS has no vibration API, but WebKit ticks
 * when a switch checkbox toggles, so iOS clicks a hidden one. Both need the
 * call inside a user gesture, such as the lift that opened the menu.
 */

const TICK_MS = 10;

export function hapticTick(doc: Document = document): void {
  const nav = doc.defaultView?.navigator;
  if (typeof nav?.vibrate === 'function') {
    nav.vibrate(TICK_MS);
    return;
  }
  // `display: none` keeps the switch unfocusable, so the tick never moves
  // focus or closes the keyboard.
  const label = doc.createElement('label');
  label.setAttribute('aria-hidden', 'true');
  label.style.display = 'none';
  const input = doc.createElement('input');
  input.type = 'checkbox';
  input.setAttribute('switch', '');
  label.appendChild(input);
  doc.head.appendChild(label);
  // A page reads a click with no press behind it as a real tap: an open
  // overlay closes on it. So the tick's clicks stop at the window. That keeps
  // the default action, and the switch still toggles.
  const view = doc.defaultView;
  const hide = (e: Event) => {
    if (label.contains(e.target as Node)) e.stopImmediatePropagation();
  };
  view?.addEventListener('click', hide, true);
  try {
    label.click();
  } finally {
    view?.removeEventListener('click', hide, true);
    label.remove();
  }
}
