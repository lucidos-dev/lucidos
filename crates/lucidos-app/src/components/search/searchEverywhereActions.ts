import { searchEverywhereOpen } from '../../store/store';
import type { FocusedPane } from '../../store/store';
import { holdSoftwareKeyboard } from '../../utils/softwareKeyboard';

export function searchResultDestinationPane(category: string): FocusedPane {
  return category === 'threads' ? 'thread' : 'content';
}

export function searchResultIconCategory(item: { category: string; id: string }): string {
  if (item.category === 'menu') return item.id;
  return item.category === 'text' ? 'files' : item.category;
}

export function focusSearchInput(): void {
  if (searchEverywhereOpen.value) return;
  holdSoftwareKeyboard();
}
