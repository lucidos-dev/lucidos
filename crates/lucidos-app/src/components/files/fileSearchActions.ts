import { fileSearchOpen, fileSearchAnchor } from '../../store/store';
import { holdSoftwareKeyboard } from '../../utils/softwareKeyboard';

/** Open the file search modal, with the keyboard up on iOS. A closed modal
 *  has no input yet, so `holdSoftwareKeyboard` keeps the keyboard until the
 *  panel focuses it. An open one (the shortcut again) refocuses its own input.
 *  Call it from the opening tap.
 *
 *  `anchor` is the toggle button that opened the modal; it's recorded so
 *  `<Overlay>` can exempt it from the outside-pointerdown dismiss. */
export function openFileSearch(anchor?: HTMLElement | null): void {
  fileSearchAnchor.value = anchor ?? null;
  fileSearchOpen.value = true;
  const input = document.querySelector<HTMLInputElement>('[data-role="file-search-input"]');
  if (input) input.focus({ preventScroll: true });
  else holdSoftwareKeyboard();
}

/** Close the file search modal. State (query, selection) lives in the modal's
 *  panel, which unmounts on close, so there is nothing else to reset here. */
export function closeFileSearch(): void {
  fileSearchOpen.value = false;
}
