/**
 * Keeping the packaged app's cursor still.
 *
 * `tao` gives its content view a cursor rect spanning the whole view, carrying
 * the window's own cursor icon, which is the arrow. AppKit re-asserts that rect
 * as the mouse moves, while WebKit sets the cursor from CSS on the same moves.
 * Two writers, one cursor, so the glyph flickers and the arrow usually wins.
 * A trackpad reports far more movement, which is where it is worst. So the page
 * tells the window what the hovered element asks for, and they stop disagreeing.
 * ADR 0129 and `src/cursor.rs` carry the rest, the upstream issues included.
 *
 * A RECONCILER, not paired enter and leave handlers: one listener answers for
 * whatever is under the pointer now. tao's rect spans the window, so a claim
 * left behind by a missed leave would be one wrong cursor over the whole app.
 *
 * This side holds NO keyword table. It forwards what CSS computed, and the one
 * table lives in Rust, so the two cannot drift apart.
 */
import { isTauri } from './platform';
import { setWindowCursor } from './tauri';

/** The bare keyword a computed `cursor` value resolves to.
 *
 *  A computed value can carry images before its fallback (`url("a.png") 2 2,
 *  pointer`), and only the last entry is a keyword. An empty value answers
 *  `auto`, the property's initial value. */
export function cursorKeyword(computed: string): string {
  const fallback = computed.split(',').pop()?.trim() ?? '';
  return fallback.split(/\s+/)[0]?.toLowerCase() || 'auto';
}

/** What the window is already showing, so a move across ten elements that all
 *  compute one keyword costs one call rather than ten. The arrow is where every
 *  window starts (`CursorIcon::Default`). */
let showing = 'default';

/** No keyword at all, so the next push always goes through. `cursorKeyword`
 *  answers `auto` for an empty value, so this can never collide with one. */
const UNKNOWN = '';

function sendOverIpc(cursor: string): Promise<void> {
  return setWindowCursor(cursor);
}

/**
 * Mirror the hovered element's CSS cursor onto the native window, for as long
 * as the document lives. A no-op off Tauri, where nothing fights WebKit.
 *
 * `send` and `doc` are test seams. Returns a teardown, which only tests call.
 */
export function installNativeCursor(
  send: (cursor: string) => Promise<void> | void = sendOverIpc,
  doc: Document = document,
): () => void {
  if (!isTauri()) return () => {};

  // The element the last answer was computed for. `pointerover` fires per
  // boundary crossing, so a repeat means the pointer re-entered the same
  // element and the style read can be skipped.
  let last: Element | null = null;

  // The cache holds what we ASKED for. A rejected call has to forget it, or a
  // crossing back to that keyword would read as already shown and leave the
  // window wrong. Forget it only while it is still the latest ask.
  //
  // Swallowed by design: a hover is not a mutating user intent, so a toast
  // would be wrong, and the crossing after this one tries again. `invoke`
  // reports a failing bridge durably through `utils/ipcHealth`.
  const show = (cursor: string) => {
    if (cursor === showing) return;
    showing = cursor;
    Promise.resolve(send(cursor)).catch(() => {
      if (showing === cursor) showing = UNKNOWN;
    });
  };

  // Capture phase, so a handler that stops propagation cannot blind this.
  const onOver = (e: Event) => {
    const target = e.target;
    if (!(target instanceof Element) || target === last) return;
    last = target;
    const style = target.ownerDocument.defaultView?.getComputedStyle(target);
    show(cursorKeyword(style?.cursor ?? ''));
  };

  // No relatedTarget means the pointer left the document itself, for the
  // browser chrome or another window. Nothing reports a crossing back in until
  // it returns, so hand the arrow back now.
  const onOut = (e: Event) => {
    if ((e as PointerEvent).relatedTarget) return;
    last = null;
    show('default');
  };

  doc.addEventListener('pointerover', onOver, true);
  doc.addEventListener('pointerout', onOut, true);
  return () => {
    doc.removeEventListener('pointerover', onOver, true);
    doc.removeEventListener('pointerout', onOut, true);
  };
}

/** Forget what the window is showing. Test seam only: the module-level cache
 *  would otherwise leak one test's cursor into the next. */
export function resetNativeCursorForTest(): void {
  showing = 'default';
}
