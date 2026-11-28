
import { NATIVE_MENU_ELEMENTS, type ContextMenuFacts } from '@lucidos/context-menu';

export function shouldOpenFilePreviewMenu(e: ContextMenuFacts, selectionAtPress: boolean): boolean {
  if (e.defaultPrevented) return false;
  if (e.altKey) return false;
  if (selectionAtPress) return false;
  const el = e.target as { closest?: (sel: string) => unknown } | null;
  if (!el || typeof el.closest !== 'function') return false;
  return el.closest(NATIVE_MENU_ELEMENTS) == null;
}
