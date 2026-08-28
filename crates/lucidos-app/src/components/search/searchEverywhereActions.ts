import { searchEverywhereOpen } from '../../store/store';
import type { FocusedPane } from '../../store/store';

/** The pane a selected search result navigates into — used to move real DOM
 *  focus there after navigating (see `handleSelect` in `SearchEverywhere`).
 *  Threads open in the conversation (thread) pane; every other category
 *  (apps, files, settings, triggers, changes, menu) lands in the content pane. */
export function searchResultDestinationPane(category: string): FocusedPane {
  return category === 'threads' ? 'thread' : 'content';
}

/** Which `CategoryIcon` glyph a result row wears.
 *
 *  A menu result is a DESTINATION rather than a kind of thing, so it wears the
 *  mark of the page it opens: the Apps row gets the apps glyph, not a glyph for
 *  "menu". Every `MenuItem` id is also a `CategoryIcon` key, the same fact
 *  `navEntryCategory` leans on. Everything else is marked by its category. */
export function searchResultIconCategory(item: { category: string; id: string }): string {
  return item.category === 'menu' ? item.id : item.category;
}

/** iOS keyboard nudge: focus a hidden input within the user-gesture call stack
 *  so the keyboard opens before the SearchEverywhere modal mounts. The modal's
 *  own input takes focus once Preact renders it. No-op when already open. */
export function focusSearchInput(): void {
  if (searchEverywhereOpen.value) return;
  const proxy = document.createElement('input');
  proxy.style.cssText = 'position:fixed;top:-9999px;left:0;opacity:0;width:1px;height:1px;';
  document.body.appendChild(proxy);
  proxy.focus();
  setTimeout(() => proxy.remove(), 500);
}
