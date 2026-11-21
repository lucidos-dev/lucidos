import { signal } from '@preact/signals';
import { threadSearchQuery, threadSearchResults } from './store';
import { closeThreadFilterPanel } from './threadFilterPanel';

/** Whether the threads-header search bar is open. One signal for both header
 *  copies and the Search threads shortcut, so a keypress opens what the button
 *  opens. */
export const threadSearchOpen = signal(false);

/** Search and the filter panel compete for the same pane body: search swaps
 *  the drawer list for its results, and the panel covers it. So opening search
 *  puts the panel away. */
export function openThreadSearch(): void {
  threadSearchOpen.value = true;
  closeThreadFilterPanel();
}

let closeCount = 0;

/** Changes on every close. A search compares it before writing results, so a
 *  response for a closed search never lands, even after a quick reopen. */
export function threadSearchSession(): number {
  return closeCount;
}

export function closeThreadSearch(): void {
  closeCount++;
  threadSearchOpen.value = false;
  threadSearchQuery.value = '';
  threadSearchResults.value = { status: 'not-loaded' };
}

const FOCUS_MAX_FRAMES = 30;

/** Put the caret in the open search bar of the layout on screen. Both header
 *  copies keep their input mounted, and the hidden layout's has no box. A
 *  drawer header fading in is still `visibility: hidden` on its first frame,
 *  where `focus()` does nothing, so it retries for a bounded number of frames. */
export function focusThreadSearchInput(frame = 0): void {
  if (!threadSearchOpen.value) return;
  for (const input of document.querySelectorAll<HTMLInputElement>('.search-active .thread-search-input')) {
    if (input.getClientRects().length === 0) continue;
    input.focus();
    if (document.activeElement === input) return;
  }
  if (frame < FOCUS_MAX_FRAMES) requestAnimationFrame(() => focusThreadSearchInput(frame + 1));
}
