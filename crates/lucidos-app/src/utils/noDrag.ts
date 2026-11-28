
import { isTauri } from './platform';

export function shouldSuppressDragStart(target: EventTarget | null): boolean {
  const el = target as { closest?: (sel: string) => unknown } | null;
  if (!el || typeof el.closest !== 'function') return true;
  return el.closest('[draggable="true"]') == null;
}

let installed = false;

export function installNoDrag(): void {
  if (installed) return;
  installed = true;
  if (!isTauri()) return;
  document.addEventListener(
    'dragstart',
    (e) => {
      if (shouldSuppressDragStart(e.target)) e.preventDefault();
    },
    { capture: true },
  );
}
