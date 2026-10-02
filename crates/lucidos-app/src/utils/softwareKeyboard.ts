import { hasCoarsePointer } from './viewport';

/** Raise the on-screen keyboard for a text field that has not mounted yet.
 *
 *  iOS opens the keyboard only for a focus inside the user's tap. A dialog or
 *  palette mounts its field a render later, outside it. So this focuses an
 *  off-screen proxy input now, and the real field takes the keyboard over when
 *  it focuses itself. Call it synchronously from the tap's handler.
 *
 *  A fine pointer has no software keyboard, so it is a no-op there. That also
 *  keeps the proxy from becoming the opener a dialog returns focus to. */
export function holdSoftwareKeyboard(): void {
  if (!hasCoarsePointer()) return;
  const proxy = document.createElement('input');
  proxy.style.cssText = 'position:fixed;top:-9999px;left:0;opacity:0;width:1px;height:1px;';
  document.body.appendChild(proxy);
  proxy.focus({ preventScroll: true });
  setTimeout(() => proxy.remove(), 500);
}
