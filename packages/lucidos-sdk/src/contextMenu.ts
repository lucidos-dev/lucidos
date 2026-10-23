/**
 * The right-click rule (ADR 0285): what counts as content, which keeps the
 * platform's own context menu.
 *
 * Shared by the host shell and by a widget frame, so the two cannot disagree.
 * The host reaches it through `@lucidos/context-menu`. Pure apart from the DOM
 * it is handed, and deliberately NOT re-exported from `index.ts`: nothing here
 * belongs on `window.lucidos`.
 */

/** A point in viewport coordinates, such as where a right-click landed. */
export interface Point {
  x: number;
  y: number;
}

/** Elements whose own native menu is worth keeping anywhere on them: an edit
 *  menu, Copy Link, Copy Image. */
export const NATIVE_MENU_ELEMENTS = [
  'input',
  'textarea',
  '[contenteditable]:not([contenteditable="false"])',
  'a[href]',
  'img',
  'video',
  'audio',
].join(', ');

/** Regions of readable text. Only the text itself keeps the native menu: the
 *  space between it gets WebKit's page menu, which is the one this removes. A
 *  new surface showing readable text opts in with `data-native-context-menu`. */
export const NATIVE_TEXT_REGIONS = [
  'pre',
  'code',
  '.markdown-content',
  '[data-native-context-menu]',
].join(', ');

/** What the decision reads from a `contextmenu` event, duck-typed so it is
 *  testable without a DOM. */
export interface ContextMenuFacts {
  target: EventTarget | null;
  altKey: boolean;
  defaultPrevented: boolean;
}

/** Whether to cancel this right-click's native menu.
 *
 *  `selectionAtPress` is whether a selection containing the pointer existed
 *  BEFORE the press. WebKit selects the word under a right-click before the
 *  event fires, so the selection at event time cannot tell the two apart.
 *  `textAtPoint` is asked only inside a text region. */
export function shouldSuppressContextMenu(
  e: ContextMenuFacts,
  selectionAtPress: boolean,
  textAtPoint: () => boolean,
): boolean {
  // A handler already claimed it for its own menu. Leave it be.
  if (e.defaultPrevented) return false;
  // The escape hatch to Inspect Element in a dev build, and to Reload.
  if (e.altKey) return false;
  if (selectionAtPress) return false;
  const el = e.target as { closest?: (sel: string) => unknown } | null;
  if (!el || typeof el.closest !== 'function') return true;
  if (el.closest(NATIVE_MENU_ELEMENTS) != null) return false;
  return el.closest(NATIVE_TEXT_REGIONS) == null || !textAtPoint();
}

type Box = { left: number; right: number; top: number; bottom: number };

function boxContains(r: Box, point: Point): boolean {
  return point.x >= r.left && point.x <= r.right && point.y >= r.top && point.y <= r.bottom;
}

/** Whether non-blank text of `target` itself is drawn under `point`. The
 *  target is the deepest element under the pointer, so text there is its own
 *  child. A caret API would snap to the nearest text instead, and at an
 *  inline boundary WebKit snaps it into the neighbour. */
export function textAt(target: EventTarget | null, point: Point, doc: Document = document): boolean {
  const children = (target as { childNodes?: ArrayLike<Node> } | null)?.childNodes;
  if (!children) return false;
  for (const node of Array.from(children)) {
    if (node.nodeType !== 3 || !node.textContent?.trim()) continue;
    const range = doc.createRange();
    range.selectNodeContents(node);
    if (Array.from(range.getClientRects()).some((r) => boxContains(r, point))) return true;
  }
  return false;
}

/** Whether the current selection covers `point`. */
export function selectionCovers(selection: Selection | null, point: Point): boolean {
  if (!selection || selection.isCollapsed) return false;
  for (let i = 0; i < selection.rangeCount; i++) {
    if (Array.from(selection.getRangeAt(i).getClientRects()).some((r) => boxContains(r, point))) return true;
  }
  return false;
}

/** A press that raises a context menu: the secondary button, or a Mac
 *  ctrl-click. */
export function opensContextMenu(e: { button: number; ctrlKey: boolean }): boolean {
  return e.button === 2 || (e.button === 0 && e.ctrlKey);
}
