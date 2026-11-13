/** Which cover was over the shell when a press began, for the dead-press probe.
 *
 *  Part of that diagnostic (`deadPressProbe.ts`, `docs/temporary-measures.md`
 *  § 1). It lives apart because its listener has to be installed before any
 *  overlay's, and the rest of the probe loads as its own chunk. */

/** Which cover the app has raised over the row, or the empty string for none.
 *
 *  An open overlay inerts the shell behind it, and a client refresh dims and
 *  locks the whole page until the reload lands. A face under either is
 *  unreachable BY DESIGN, so no reading taken under one means anything.
 *
 *  It answers with a NAME rather than a flag, because declining to judge is not
 *  a reason to say nothing. A press the probe refused under a cover writes a
 *  line carrying which cover refused it. */
export function coverOverShell(): string {
  const root = document.documentElement;
  const open = root.hasAttribute('data-overlay-open');
  const blocked = root.hasAttribute('data-ui-blocked');
  if (open && blocked) return 'data-overlay-open data-ui-blocked';
  if (open) return 'data-overlay-open';
  if (blocked) return 'data-ui-blocked';
  return '';
}

/** The cover as it stood when the current gesture's `pointerdown` arrived.
 *
 *  WebKit fires `pointerdown` before `touchstart`, and an overlay dismisses on
 *  the first. So a tap that closes a menu reaches the `touchstart` with the
 *  cover already gone. Its touch was hit-tested under the cover and went to the
 *  shell, while Send answers at the point. That is a swallowed dismiss, and it
 *  read as a dead Send.
 *
 *  Valid only while this listener runs before any overlay's. Shell startup
 *  installs it synchronously, and an overlay adds its listener when it opens. */
let coverAtPointerDown = '';
let installed = false;

/** Start sampling the cover at each gesture's `pointerdown`. Idempotent. */
export function installPressCoverWatch(): void {
  if (installed || typeof document === 'undefined') return;
  installed = true;
  // A second finger is not a new gesture, so it keeps the first one's reading.
  document.addEventListener('pointerdown', (e) => {
    if (e.isPrimary === false) return;
    coverAtPointerDown = coverOverShell();
  }, { capture: true, passive: true });
}

/** Which cover was over the shell for the press now being dispatched.
 *
 *  Consuming, so the reading belongs to one gesture. A `touchstart` with no
 *  `pointerdown` of its own must not inherit an earlier dismiss's cover, or a
 *  real dead press would log as covered. */
export function takeCoverForThisPress(): string {
  const cover = coverOverShell() || coverAtPointerDown;
  coverAtPointerDown = '';
  return cover;
}
