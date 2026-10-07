import { searchEverywhereOpen } from '../../store/store';
import type { FocusedPane } from '../../store/store';
import { holdSoftwareKeyboard } from '../../utils/softwareKeyboard';

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
 *  `navEntryCategory` leans on. A Text line wears the file it is in. Everything
 *  else is marked by its category. */
export function searchResultIconCategory(item: { category: string; id: string }): string {
  if (item.category === 'menu') return item.id;
  return item.category === 'text' ? 'files' : item.category;
}

/** Open the keyboard before the SearchEverywhere modal mounts its input. Call
 *  it from the tap that opens the modal. No-op when already open. */
export function focusSearchInput(): void {
  if (searchEverywhereOpen.value) return;
  holdSoftwareKeyboard();
}
