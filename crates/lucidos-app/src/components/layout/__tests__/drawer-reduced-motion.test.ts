import { beforeAll, afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { closeDrawer, drawerClosing, drawerOpen, forceCloseDrawer, openDrawer } from '../drawerState';
import { motionPreference, osReducesMotion } from '../../../utils/motion';
import type { MotionPref } from '@lucidos/appearance';
import { DrawerChunk } from '../drawerState';

beforeAll(() => DrawerChunk.preload());

function setMotion(pref: MotionPref, osReduces: boolean): void {
  motionPreference.value = pref;
  osReducesMotion.value = osReduces;
}

beforeEach(() => {
  forceCloseDrawer();
});

afterEach(() => {
  setMotion('system', false);
  forceCloseDrawer();
});

const REDUCED: Array<[MotionPref, boolean]> = [['system', true], ['reduce', false], ['reduce', true]];

describe('closing the menu drawer under reduced motion', () => {
  for (const [pref, os] of REDUCED) {
    it(`reaches the closed state with no animationend (${pref}, OS ${os ? 'on' : 'off'})`, () => {
      setMotion(pref, os);
      openDrawer();

      expect(closeDrawer()).toBe(true);

      expect(drawerOpen.value).toBe(false);
      expect(drawerClosing.value).toBe(false);
    });
  }

  it('leaves no stuck overlay for the rest of the session', () => {
    setMotion('reduce', false);
    openDrawer();
    closeDrawer();

    expect(drawerOpen.value).toBe(false);
    openDrawer();
    expect(drawerOpen.value).toBe(true);
    expect(drawerClosing.value).toBe(false);
  });

  it('still reports the already-closed case as a no-op', () => {
    setMotion('reduce', false);

    expect(closeDrawer()).toBe(false);
    expect(drawerOpen.value).toBe(false);
  });
});

describe('closing the menu drawer with motion on', () => {
  it('waits for the slide-out rather than closing straight through', () => {
    setMotion('system', false);
    openDrawer();

    expect(closeDrawer()).toBe(true);

    expect(drawerClosing.value).toBe(true);
    expect(drawerOpen.value).toBe(true);
  });

  it('waits for it under Full even when the OS asks to reduce', () => {
    setMotion('full', true);
    openDrawer();

    expect(closeDrawer()).toBe(true);
    expect(drawerClosing.value).toBe(true);
  });

  it('closes on its fallback timer if the slide-out end never arrives', () => {
    vi.useFakeTimers();
    try {
      setMotion('system', false);
      openDrawer();
      closeDrawer();
      osReducesMotion.value = true;

      vi.advanceTimersByTime(300);

      expect(drawerOpen.value).toBe(false);
      expect(drawerClosing.value).toBe(false);
    } finally {
      vi.useRealTimers();
    }
  });

  it('does not let a stale fallback cut a later close short', () => {
    vi.useFakeTimers();
    try {
      setMotion('system', false);
      openDrawer();
      closeDrawer();
      vi.advanceTimersByTime(250);
      openDrawer();
      closeDrawer();
      vi.advanceTimersByTime(60);
      expect(drawerClosing.value).toBe(true);
    } finally {
      vi.useRealTimers();
    }
  });

  it('reports a second call mid-slide-out as a no-op', () => {
    setMotion('system', false);
    openDrawer();
    closeDrawer();

    expect(closeDrawer()).toBe(false);
  });
});
