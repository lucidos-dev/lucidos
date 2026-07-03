import { fileSearchOpen, fileSearchAnchor } from '../../store/store';

/** Open the file search modal and grab focus from the user-gesture call stack
 *  so iOS opens the keyboard. The real input only mounts after this render
 *  (the closed overlay renders a bare hidden shell with no input), so we focus
 *  a throwaway proxy input now to hold the keyboard open within the gesture
 *  window; the panel's own auto-focus takes over once Preact mounts the real
 *  input. (The `if (input)` fast-path stays as a harmless guard in case a real
 *  input is ever already present.)
 *
 *  `anchor` is the toggle button that opened the modal; it's recorded so
 *  `<Overlay>` can exempt it from the outside-pointerdown dismiss. */
export function openFileSearch(anchor?: HTMLElement | null): void {
  fileSearchAnchor.value = anchor ?? null;
  fileSearchOpen.value = true;
  const input = document.querySelector<HTMLInputElement>('[data-role="file-search-input"]');
  if (input) {
    input.focus({ preventScroll: true });
    return;
  }
  const proxy = document.createElement('input');
  proxy.style.cssText = 'position:fixed;top:-9999px;left:0;opacity:0;width:1px;height:1px;';
  document.body.appendChild(proxy);
  proxy.focus({ preventScroll: true });
  setTimeout(() => proxy.remove(), 500);
}

/** Close the file search modal. State (query, selection) lives in the modal's
 *  panel, which unmounts on close, so there is nothing else to reset here. */
export function closeFileSearch(): void {
  fileSearchOpen.value = false;
}
