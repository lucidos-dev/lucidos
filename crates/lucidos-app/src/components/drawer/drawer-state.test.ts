import { beforeAll, describe, it, expect, beforeEach } from 'vitest';
import { closeDrawer, drawerOpen, drawerClosing, openDrawer } from '../layout/drawerState';
import { DrawerChunk } from '../layout/drawerState';

// The drawer is code-split and opens once it is in memory. Warm it, so an
// open lands at once.
beforeAll(() => DrawerChunk.preload());

/** Simulate the burger click handler logic from AppHeader */
function burgerClick() {
  if (drawerOpen.value && !drawerClosing.value) closeDrawer();
  else openDrawer();
}

describe('Drawer state machine', () => {
  beforeEach(() => {
    drawerOpen.value = false;
    drawerClosing.value = false;
  });

  it('closeDrawer sets drawerClosing to true', () => {
    drawerOpen.value = true;
    closeDrawer();
    expect(drawerClosing.value).toBe(true);
  });

  it('closeDrawer is a no-op when drawer is already closed', () => {
    closeDrawer();
    expect(drawerClosing.value).toBe(false);
  });

  it('openDrawer resets drawerClosing', () => {
    drawerClosing.value = true;
    openDrawer();
    expect(drawerOpen.value).toBe(true);
    expect(drawerClosing.value).toBe(false);
  });

  it('burger click opens drawer when closed', () => {
    burgerClick();
    expect(drawerOpen.value).toBe(true);
    expect(drawerClosing.value).toBe(false);
  });

  it('burger click closes drawer when open', () => {
    drawerOpen.value = true;
    burgerClick();
    expect(drawerClosing.value).toBe(true);
  });

  it('burger click recovers from stuck closing state (onAnimationEnd never fired)', () => {
    // Simulate: closeDrawer() was called, animation started but onAnimationEnd never fired
    // State is stuck: drawerOpen=true, drawerClosing=true
    drawerOpen.value = true;
    drawerClosing.value = true;

    // User clicks burger — should recover by opening the drawer
    burgerClick();

    expect(drawerOpen.value).toBe(true);
    expect(drawerClosing.value).toBe(false);
  });
});
