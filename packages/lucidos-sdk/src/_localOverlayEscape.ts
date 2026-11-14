import { dismissTopOverlay } from '../../../crates/lucidos-app/src/store/overlayStack';

const consumed = new WeakSet<KeyboardEvent>();
let instances = 0;
/** Called before the iframe forwards a shortcut to its host. */
export function consumeLocalOverlayEscape(event: KeyboardEvent): boolean {
  if (consumed.has(event)) return true;
  if (instances === 0 || event.key !== 'Escape' || !dismissTopOverlay()) return false;
  consumed.add(event);
  event.preventDefault();
  event.stopImmediatePropagation();
  return true;
}
function onKeyDown(event: KeyboardEvent) { consumeLocalOverlayEscape(event); }
export function installLocalOverlayEscape(): () => void {
  if (instances++ === 0) window.addEventListener('keydown', onKeyDown, true);
  let active = true;
  return () => {
    if (!active) return;
    active = false;
    if (--instances === 0) window.removeEventListener('keydown', onKeyDown, true);
  };
}
