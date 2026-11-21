/**
 * The menu drawer opens only once its chunk is in memory (ADR 0436). Until
 * then, a forced close or a second open takes the waiting open back. So the
 * drawer never slides in over whatever came next.
 */
import { describe, it, expect, vi } from 'vitest';

const chunk = vi.hoisted(() => {
  let release: () => void = () => {};
  const arrived = new Promise<void>((resolve) => { release = resolve; });
  return { arrived, release: () => release() };
});
vi.mock('../Drawer', async () => {
  await chunk.arrived;
  return { Drawer: () => null };
});

const { DrawerChunk, drawerOpen, forceCloseDrawer, openDrawer } = await import('../drawerState');

const settle = () => new Promise((r) => setTimeout(r, 0));

describe('opening the menu drawer before its chunk arrives', () => {
  it('takes the open back on a forced close and on a second open', async () => {
    openDrawer();
    expect(drawerOpen.value, 'nothing opens before the chunk arrives').toBe(false);
    forceCloseDrawer();
    openDrawer();
    openDrawer();
    chunk.release();
    await DrawerChunk.preload();
    await settle();
    expect(drawerOpen.value).toBe(false);
  });

  it('opens at once once the chunk is in memory', () => {
    openDrawer();
    expect(drawerOpen.value).toBe(true);
    forceCloseDrawer();
  });
});
