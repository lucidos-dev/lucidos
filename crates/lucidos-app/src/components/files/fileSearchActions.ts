import { fileSearchOpen } from '../../store/store';

/** Open the file search modal and focus the input from the user-gesture call
 *  stack so iOS opens the keyboard. Subsequent opens hit the modal's existing
 *  hidden-shell input directly; cold opens (when the modal chunk hasn't loaded
 *  yet) fall back to a proxy input — the modal's own auto-focus takes over
 *  once Preact mounts the real one and iOS keeps the keyboard open. */
export function openFileSearch(): void {
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
