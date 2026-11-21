// Kill WebKit's native content drag in the Tauri desktop build.
//
// WHAT THE USER SEES: dragging the window by the header sometimes flashes a
// green "+" copy badge plus a translucent ghost-mirror of the text under the
// cursor. That is WKWebView starting an HTML5 drag-and-drop of the content (a
// text selection / link / image) at the same time the press becomes a window
// drag — a spurious "copy this content" gesture the app never wants.
//
// WHY A dragstart SUPPRESSOR: `user-select: none` on the header stops a text
// *selection* from forming, but WebKit can still initiate a native drag from
// other content the gesture grazes, and the ghost reads as broken next to the
// window move. Cancelling the `dragstart` itself is the root-cause fix — it stops
// the drag regardless of whether the source is text, an image, or a link.
//
// SAFE BY CONSTRUCTION: the app has no internal HTML5 drag feature (no
// `draggable="true"` elements, no reorder-by-drag). The only drag-and-drop is
// dropping OS files INTO the window (files/DropZone, the picker's restore drop),
// which fires `dragenter`/`dragover`/`drop` — never `dragstart`, because the
// drag originates in Finder, not in a DOM node here. So suppressing `dragstart`
// cannot break file drops. We still honour an explicit `draggable="true"`
// opt-out for any future drag source.
//
// SCOPED TO TAURI: web/PWA users can still drag a selection out to another app
// (a niche but real browser affordance); the broken-looking overlap only happens
// in the desktop window, so the gate keeps the fix where the bug is.

import { isTauri } from './platform';

/** Whether a `dragstart` from this target should be cancelled. Everything is
 *  cancelled except elements that explicitly opt into native drag via
 *  `draggable="true"`. Duck-typed on `closest` so it's testable without a real
 *  DOM (and realm-agnostic): a non-element target (text node, document, null)
 *  has no `closest` → suppress. */
export function shouldSuppressDragStart(target: EventTarget | null): boolean {
  const el = target as { closest?: (sel: string) => unknown } | null;
  if (!el || typeof el.closest !== 'function') return true;
  return el.closest('[draggable="true"]') == null;
}

let installed = false;

/** Install the global native-drag suppressor. Idempotent; a no-op off Tauri
 *  (web / PWA / mobile keep their normal drag behavior). Capture phase so the
 *  cancel lands before any element-level `dragstart` handler. */
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
