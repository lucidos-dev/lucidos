import { searchEverywhereOpen } from '../../store/store';

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
