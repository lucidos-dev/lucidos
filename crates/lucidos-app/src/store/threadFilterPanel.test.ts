import { describe, it, expect, beforeEach } from 'vitest';
import {
  threadFilterPanelOpen,
  openThreadFilterPanel,
  closeThreadFilterPanel,
  toggleThreadFilterPanel,
} from './threadFilterPanel';
import { overlayStack, dismissTopOverlay, pushOverlay, _resetOverlayStackForTesting } from './overlayStack';

/** The filter panel is a view inside the thread drawer pane, not an `<Overlay>`:
 *  it does not dismiss on an outside click and makes nothing inert. Escape is
 *  the one overlay behavior it keeps, and Escape has a single owner app-wide
 *  (the LIFO `overlayStack`), so the panel registers there while open. These
 *  tests pin that the registration follows the state exactly. The reload half
 *  (restoring open, and taking an Escape entry when it does) is in
 *  `threadFilterPanel-persistence.test.ts`, which needs a fresh module per case.
 *
 *  Reset through the real closer rather than by writing the signal, so the
 *  persisted key is cleared with it and no case inherits the previous one's. */
beforeEach(() => {
  closeThreadFilterPanel();
  _resetOverlayStackForTesting();
});

describe('thread filter panel state', () => {
  it('opens and closes', () => {
    openThreadFilterPanel();
    expect(threadFilterPanelOpen.value).toBe(true);
    closeThreadFilterPanel();
    expect(threadFilterPanelOpen.value).toBe(false);
  });

  it('toggles from either state', () => {
    toggleThreadFilterPanel();
    expect(threadFilterPanelOpen.value).toBe(true);
    toggleThreadFilterPanel();
    expect(threadFilterPanelOpen.value).toBe(false);
  });

  it('registers on the Escape stack while open and leaves nothing behind on close', () => {
    openThreadFilterPanel();
    expect(overlayStack.value.map(e => e.id)).toEqual(['thread-filter-panel']);
    closeThreadFilterPanel();
    expect(overlayStack.value).toHaveLength(0);
  });

  it('closes when Escape pops the top of the stack', () => {
    openThreadFilterPanel();
    expect(dismissTopOverlay()).toBe(true);
    expect(threadFilterPanelOpen.value).toBe(false);
    expect(overlayStack.value).toHaveLength(0);
  });

  it('leaves a genuine overlay opened on top of it to take Escape first', () => {
    // A row's overflow menu opened while the panel is up is newer, so LIFO must
    // close the menu and leave the panel standing.
    openThreadFilterPanel();
    let menuClosed = false;
    pushOverlay({ id: 'menu', dismiss: () => { menuClosed = true; } });
    dismissTopOverlay();
    expect(menuClosed).toBe(true);
    expect(threadFilterPanelOpen.value).toBe(true);
  });

  it('does not double-register when opened twice', () => {
    openThreadFilterPanel();
    openThreadFilterPanel();
    expect(overlayStack.value).toHaveLength(1);
    closeThreadFilterPanel();
    expect(overlayStack.value).toHaveLength(0);
  });
});
