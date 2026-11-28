import { signal } from '@preact/signals';
import { threadSearchQuery, threadSearchResults } from './store';
import { closeThreadFilterPanel } from './threadFilterPanel';

export const threadSearchOpen = signal(false);

export function openThreadSearch(): void {
  threadSearchOpen.value = true;
  closeThreadFilterPanel();
}

let closeCount = 0;

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

export function focusThreadSearchInput(frame = 0): void {
  if (!threadSearchOpen.value) return;
  for (const input of document.querySelectorAll<HTMLInputElement>('.search-active .thread-search-input')) {
    if (input.getClientRects().length === 0) continue;
    input.focus();
    if (document.activeElement === input) return;
  }
  if (frame < FOCUS_MAX_FRAMES) requestAnimationFrame(() => focusThreadSearchInput(frame + 1));
}
