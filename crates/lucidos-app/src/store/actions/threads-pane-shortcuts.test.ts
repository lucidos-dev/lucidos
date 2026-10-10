// @vitest-environment jsdom
/** The thread filter, thread search and menu drawer shortcuts.
 *
 *  Plan: `docs/plans/2026-10-02-shortcut-coverage-and-search-landing.md`. */
import { describe, it, expect, beforeEach } from 'vitest';
import { mobileView, threadDrawerOpen, splitRatio, focusedPane, threadSearchQuery } from '../store';
import { threadFilterPanelOpen, closeThreadFilterPanel, openThreadFilterPanel } from '../threadFilterPanel';
import { threadSearchOpen, openThreadSearch } from '../threadSearch';
import { drawerOpen, drawerClosing, drawerAnchor, openDrawer } from '../../components/layout/drawerState';
import { toggleMenuDrawer } from '../../components/layout/ContentNav';
import { toggleThreadFilter, startThreadSearch } from './pane';

function setViewport(px: number): void {
  Object.defineProperty(window, 'innerWidth', { value: px, configurable: true, writable: true });
}

beforeEach(() => {
  setViewport(1280);
  mobileView.value = 'thread';
  threadDrawerOpen.value = true;
  splitRatio.value = 0.4;
  focusedPane.value = 'thread';
  closeThreadFilterPanel();
  threadSearchOpen.value = false;
  threadSearchQuery.value = '';
  drawerOpen.value = false;
  drawerClosing.value = false;
  drawerAnchor.value = null;
});

describe('toggleThreadFilter', () => {
  it('toggles the filter on a shown thread list, and the drawer takes focus', () => {
    toggleThreadFilter();
    expect(threadFilterPanelOpen.value).toBe(true);
    expect(focusedPane.value).toBe('drawer');
    toggleThreadFilter();
    expect(threadFilterPanelOpen.value).toBe(false);
  });

  it('brings a closed drawer back with the filter up', () => {
    threadDrawerOpen.value = false;
    toggleThreadFilter();
    expect(threadDrawerOpen.value).toBe(true);
    expect(threadFilterPanelOpen.value).toBe(true);
  });

  it('shows the filter rather than shutting it when the list was hidden', () => {
    // A filter left open behind a collapsed Conversation side is what the
    // user expects to see, not to close unseen.
    openThreadFilterPanel();
    splitRatio.value = 0;
    toggleThreadFilter();
    expect(splitRatio.value).toBeGreaterThan(0);
    expect(threadFilterPanelOpen.value).toBe(true);
  });

  it('swipes to the threads pane on mobile', () => {
    setViewport(375);
    toggleThreadFilter();
    expect(mobileView.value).toBe('threads');
    expect(threadFilterPanelOpen.value).toBe(true);
  });

  it('puts thread search away when it shows the filter', () => {
    openThreadSearch();
    threadSearchQuery.value = 'deploy';
    toggleThreadFilter();
    expect(threadSearchOpen.value).toBe(false);
    expect(threadSearchQuery.value).toBe('');
  });
});

describe('startThreadSearch', () => {
  it('opens search on a revealed thread list and puts the filter away', () => {
    threadDrawerOpen.value = false;
    openThreadFilterPanel();
    startThreadSearch();
    expect(threadDrawerOpen.value).toBe(true);
    expect(threadSearchOpen.value).toBe(true);
    expect(threadFilterPanelOpen.value).toBe(false);
    expect(focusedPane.value).toBe('drawer');
  });
});

describe('toggleMenuDrawer', () => {
  it('opens with no anchor from the shortcut, clearing a past press', () => {
    const button = document.createElement('button');
    openDrawer(button);
    drawerOpen.value = false;
    toggleMenuDrawer();
    expect(drawerOpen.value).toBe(true);
    expect(drawerAnchor.value).toBeNull();
  });

  it('closes an open drawer', () => {
    toggleMenuDrawer();
    toggleMenuDrawer();
    expect(drawerOpen.value && !drawerClosing.value).toBe(false);
  });
});
