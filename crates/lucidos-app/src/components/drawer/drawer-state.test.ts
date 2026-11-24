import { beforeAll, describe, it, expect, beforeEach } from 'vitest';
import { closeDrawer, drawerOpen, drawerClosing, openDrawer } from '../layout/drawerState';
import { DrawerChunk } from '../layout/drawerState';

beforeAll(() => DrawerChunk.preload());

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
    drawerOpen.value = true;
    drawerClosing.value = true;

    burgerClick();

    expect(drawerOpen.value).toBe(true);
    expect(drawerClosing.value).toBe(false);
  });
});
