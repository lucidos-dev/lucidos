import { fileSearchOpen, fileSearchAnchor } from '../../store/store';
import { holdSoftwareKeyboard } from '../../utils/softwareKeyboard';

export function openFileSearch(anchor?: HTMLElement | null): void {
  fileSearchAnchor.value = anchor ?? null;
  fileSearchOpen.value = true;
  const input = document.querySelector<HTMLInputElement>('[data-role="file-search-input"]');
  if (input) input.focus({ preventScroll: true });
  else holdSoftwareKeyboard();
}

export function closeFileSearch(): void {
  fileSearchOpen.value = false;
}
