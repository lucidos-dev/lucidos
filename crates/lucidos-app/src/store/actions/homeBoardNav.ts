/** Landing on the Home board (ADR 0432). Kept apart from the board's reads, so
 *  the SSE path never pulls the menu's loaders into the entry chunk. */

import { landNextHomeOnBoard } from '../homeBoard';
import { showHomeRail } from './pane';
import { openHomeThread } from './homeThread';
import { viewportIsMobile } from '../../utils/viewport';

/** A tap on the Home icon, or the Lucidos menu's Home row. On desktop it opens
 *  the Home chat only, even in Home: the rail's own toggle shows the board. On a
 *  phone it opens Home on its board face, sliding the chat down if it was up. */
export function openHome(): void {
  if (!viewportIsMobile.value) {
    openHomeThread();
    return;
  }
  landNextHomeOnBoard();
  openHomeThread();
  // The keyboard belongs to the chat face.
  (document.activeElement as HTMLElement | null)?.blur?.();
}

/** The board wherever this layout keeps it: the rail on desktop, shown if it
 *  was hidden, and Home's board face on a phone. */
export function revealHomeBoard(): void {
  if (viewportIsMobile.value) openHome();
  else showHomeRail();
}
