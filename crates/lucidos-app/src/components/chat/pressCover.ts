/** Which cover was over the shell when a press began, for the dead-press probe.
 *
 *  Part of that diagnostic (`deadPressProbe.ts`, `docs/temporary-measures.md`
 *  § 1). It lives apart because its listener has to be installed before any
 *  overlay's, and the rest of the probe loads as its own chunk. */

export function coverOverShell(): string {
  const root = document.documentElement;
  const open = root.hasAttribute('data-overlay-open');
  const blocked = root.hasAttribute('data-ui-blocked');
  if (open && blocked) return 'data-overlay-open data-ui-blocked';
  if (open) return 'data-overlay-open';
  if (blocked) return 'data-ui-blocked';
  return '';
}

let coverAtPointerDown = '';
let installed = false;

export function installPressCoverWatch(): void {
  if (installed || typeof document === 'undefined') return;
  installed = true;
  document.addEventListener('pointerdown', (e) => {
    if (e.isPrimary === false) return;
    coverAtPointerDown = coverOverShell();
  }, { capture: true, passive: true });
}

export function takeCoverForThisPress(): string {
  const cover = coverOverShell() || coverAtPointerDown;
  coverAtPointerDown = '';
  return cover;
}
