import { describe, it, expect, beforeEach } from 'vitest';
import {
  threadFilterPanelOpen,
  openThreadFilterPanel,
  closeThreadFilterPanel,
  toggleThreadFilterPanel,
  setThreadFilterPaneVisible,
} from './threadFilterPanel';
import { overlayStack, dismissTopOverlay, pushOverlay, _resetOverlayStackForTesting } from './overlayStack';

beforeEach(() => {
  setThreadFilterPaneVisible(true);
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
    openThreadFilterPanel();
    let menuClosed = false;
    pushOverlay({ id: 'menu', dismiss: () => { menuClosed = true; }, hasPanel: true });
    dismissTopOverlay();
    expect(menuClosed).toBe(true);
    expect(threadFilterPanelOpen.value).toBe(true);
  });

  it('stays open while the drawer is collapsed, but takes no Escape until it shows again', () => {
    openThreadFilterPanel();
    setThreadFilterPaneVisible(false);
    expect(threadFilterPanelOpen.value).toBe(true);
    expect(overlayStack.value).toHaveLength(0);
    setThreadFilterPaneVisible(true);
    expect(overlayStack.value.map(e => e.id)).toEqual(['thread-filter-panel']);
  });

  it('does not register a panel opened while the drawer is collapsed', () => {
    setThreadFilterPaneVisible(false);
    openThreadFilterPanel();
    expect(overlayStack.value).toHaveLength(0);
  });

  it('does not double-register when opened twice', () => {
    openThreadFilterPanel();
    openThreadFilterPanel();
    expect(overlayStack.value).toHaveLength(1);
    closeThreadFilterPanel();
    expect(overlayStack.value).toHaveLength(0);
  });
});
