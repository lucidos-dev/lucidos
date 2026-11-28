
const TICK_MS = 10;

export function hapticTick(doc: Document = document): void {
  const nav = doc.defaultView?.navigator;
  if (typeof nav?.vibrate === 'function') {
    nav.vibrate(TICK_MS);
    return;
  }
  const label = doc.createElement('label');
  label.setAttribute('aria-hidden', 'true');
  label.style.display = 'none';
  const input = doc.createElement('input');
  input.type = 'checkbox';
  input.setAttribute('switch', '');
  label.appendChild(input);
  doc.head.appendChild(label);
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
