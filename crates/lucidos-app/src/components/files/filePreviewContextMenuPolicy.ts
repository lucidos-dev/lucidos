// Whether a right-click over the file-preview content pane should open its
// own actions menu (ADR 0285: an object with actions claims the event first)
// or defer to whatever the platform/browser would otherwise show.
//
// Unlike `utils/nativeContextMenu.ts`'s Tauri-only page-menu policy, this
// runs on every platform: a browser tab's right-click belongs to the page
// here, because the preview IS the object with its own menu. The two never
// overlap, since this one never touches chrome outside the preview body.

import { NATIVE_MENU_ELEMENTS, type ContextMenuFacts } from '../../utils/nativeContextMenu';

/** Whether this right-click should open the file-preview context menu.
 *
 *  `selectionAtPress` is whether a selection containing the pointer existed
 *  BEFORE the press, exactly as `nativeContextMenu.ts` tracks it. WebKit
 *  selects the word under a right-click before `contextmenu` fires. So the
 *  selection at event time cannot tell a fresh auto-select from one the user
 *  made on purpose.
 *
 *  Unlike that module's `shouldSuppressContextMenu`, there is no text-region
 *  carve-out here. An ordinary word with no selection still opens THIS menu:
 *  that is the whole point, the Obsidian-style right-click. Only a selection
 *  already under the pointer, or a native-menu element (a link, an image, an
 *  input, the active editor), defers. */
export function shouldOpenFilePreviewMenu(e: ContextMenuFacts, selectionAtPress: boolean): boolean {
  // A handler already claimed it (or this press raised no menu at all).
  if (e.defaultPrevented) return false;
  // The escape hatch to the native/dev-tools menu, same as everywhere else.
  if (e.altKey) return false;
  if (selectionAtPress) return false;
  const el = e.target as { closest?: (sel: string) => unknown } | null;
  if (!el || typeof el.closest !== 'function') return false;
  return el.closest(NATIVE_MENU_ELEMENTS) == null;
}
