
export interface Point {
  x: number;
  y: number;
}

export const NATIVE_MENU_ELEMENTS = [
  'input',
  'textarea',
  '[contenteditable]:not([contenteditable="false"])',
  'a[href]',
  'img',
  'video',
  'audio',
].join(', ');

export const NATIVE_TEXT_REGIONS = [
  'pre',
  'code',
  '.markdown-content',
  '[data-native-context-menu]',
].join(', ');

export interface ContextMenuFacts {
  target: EventTarget | null;
  altKey: boolean;
  defaultPrevented: boolean;
}

export function shouldSuppressContextMenu(
  e: ContextMenuFacts,
  selectionAtPress: boolean,
  textAtPoint: () => boolean,
): boolean {
  if (e.defaultPrevented) return false;
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

export function selectionCovers(selection: Selection | null, point: Point): boolean {
  if (!selection || selection.isCollapsed) return false;
  for (let i = 0; i < selection.rangeCount; i++) {
    if (Array.from(selection.getRangeAt(i).getClientRects()).some((r) => boxContains(r, point))) return true;
  }
  return false;
}

export function opensContextMenu(e: { button: number; ctrlKey: boolean }): boolean {
  return e.button === 2 || (e.button === 0 && e.ctrlKey);
}
