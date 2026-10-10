/**
 * The menu drawer must not depend on an animation that may not run.
 *
 * `closeDrawer()` normally only sets `drawerClosing`, and the panel's
 * `onAnimationEnd` handler is what clears `drawerOpen`. Under reduced motion
 * the CSS drops the animation on `.drawer.closing` (mobile.css), and an element
 * with no animation fires no `animationend`. So the drawer stayed open, and
 * `<Overlay open>` held `data-overlay-open` on `<html>`, which inerts the whole
 * shell behind it.
 *
 * The in-app Motion setting makes that path common, and it can disagree with
 * the OS in both directions. So these cases drive the resolved value from both
 * inputs. They dispatch no `animationend` at all and assert synchronously: no
 * event and no timer may stand between the call and the closed state.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { closeDrawer } from '../Drawer';
import { drawerClosing, drawerOpen, forceCloseDrawer, openDrawer } from '../drawerState';
import { motionPreference, osReducesMotion } from '../../../utils/motion';
import type { MotionPref } from '@lucidos/appearance';

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

/** Every input pair that resolves to reduced motion. The middle one is the case
 *  a media-query read would miss: calm chosen in the app, OS switch off. */
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

    // A stuck `drawerOpen` keeps `<Overlay open>` mounted, which is what inerts
    // the thread list, the composer and every header control but the hamburger.
    expect(drawerOpen.value).toBe(false);
    // And the hamburger must reopen rather than toggle a panel that never left.
    openDrawer();
    expect(drawerOpen.value).toBe(true);
    expect(drawerClosing.value).toBe(false);
  });

  it('still reports the already-closed case as a no-op', () => {
    // The load-bearing `false`: the dismiss hook leaves the paired click
    // un-swallowed, so a tap on a neighbor button still reaches its handler.
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
    // `full` keeps the slide-out, so the animation runs and its end event fires.
    setMotion('full', true);
    openDrawer();

    expect(closeDrawer()).toBe(true);
    expect(drawerClosing.value).toBe(true);
  });

  it('closes on its fallback timer if the slide-out end never arrives', () => {
    // Motion turning reduced mid-slide drops the running animation, so its
    // `animationend` never fires. The timer is what keeps the shell usable.
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
      // The first close's timer fires here; the second close is still sliding.
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
