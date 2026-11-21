/**
 * The one number the shell cannot compute for itself.
 *
 * The macOS traffic lights are centred on our header bar by
 * `src/traffic_lights.rs`, but the bar's height is `--titlebar-inset` plus a
 * rem-authored `--app-header-height`, so it depends on the user's UI scale and
 * exists only in the page. These pin the three properties that make the push
 * correct. It follows the rendered band, rather than a list of the applies that
 * move it. It measures that band, rather than restating `3rem`. And it does not
 * fire at all on a build with no native lights.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

const platformMocks = { isTauri: true };
vi.mock('../../utils/platform', () => ({
  isTauri: () => platformMocks.isTauri,
  isIOS: () => false,
  isIOSPwa: () => false,
}));

const setTrafficLightOffsetMock = vi.hoisted(() => vi.fn(() => Promise.resolve()));
vi.mock('../../utils/tauri', () => ({
  setTrafficLightOffset: setTrafficLightOffsetMock,
}));

import {
  measureHeaderBarHeight, pushTrafficLightOffset, resetTrafficLightPush,
  watchTitlebarBand, TITLEBAR_BAND_SELECTOR,
} from './trafficLights';

/** The header's bottom edge below the viewport's top, which under the overlay
 *  build is the window's top. `null` for no header mounted at all. There is no
 *  layout engine in this harness (see src/test-setup.ts), so the rect is stubbed
 *  the way a real one would resolve. */
let headerBottom: number | null = 48;
/** Whether the document carries `data-titlebar-overlay`, i.e. whether this is a
 *  window with native traffic lights on it. */
let overlayBuild = true;

/** The header's own layout height, which no transform touches. Defaults to the
 *  desktop shape (a 28px strip above a 20px header). */
let headerOffsetHeight = 20;

const mountHeader = (bottom: number | null, offsetHeight = 20): void => {
  headerBottom = bottom;
  headerOffsetHeight = offsetHeight;
};

beforeEach(() => {
  platformMocks.isTauri = true;
  overlayBuild = true;
  headerBottom = 48;
  headerOffsetHeight = 20;
  vi.spyOn(document, 'querySelector').mockImplementation((selector: string) =>
    selector === TITLEBAR_BAND_SELECTOR && headerBottom !== null
      ? ({
        getBoundingClientRect: () => ({ bottom: headerBottom }),
        offsetHeight: headerOffsetHeight,
      } as unknown as Element)
      : null,
  );
  vi.spyOn(document.documentElement, 'hasAttribute').mockImplementation(
    (name: string) => name === 'data-titlebar-overlay' && overlayBuild,
  );
  setTrafficLightOffsetMock.mockClear();
  resetTrafficLightPush();
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('measureHeaderBarHeight', () => {
  it('is the rendered header\'s bottom edge, not a restated 3rem', () => {
    // ONE read, and it is the only value that cannot drift from the CSS: the
    // strip is a flow sibling above the header, so the header's bottom IS
    // --titlebar-inset + --app-header-height whatever those resolve to.
    mountHeader(72);
    expect(measureHeaderBarHeight()).toBe(72);
  });

  it('answers null when no surface has declared a band to measure', () => {
    // A push with nothing measured must not invent a bar. The pre-gateway boot
    // splash is the surface that legitimately declares none.
    mountHeader(null);
    expect(measureHeaderBarHeight()).toBeNull();
  });

  it('answers null for a header that has not been laid out', () => {
    mountHeader(0);
    expect(measureHeaderBarHeight()).toBeNull();
  });

  it('answers null for a header translated away by hide-on-scroll', () => {
    // The mobile layout, which a packaged macOS window narrower than 769px gets
    // with real lights still on it: the header is fixed at top 0, so at rest its
    // painted bottom equals its own height, and anything less means
    // `useHideOnScroll` has translated it up. Centring on that would put the
    // lights above the bar and the de-duplication would hold the bad reading.
    mountHeader(44, 44);
    expect(measureHeaderBarHeight(), 'at rest the whole header IS the bar').toBe(44);
    mountHeader(19, 44);
    expect(measureHeaderBarHeight(), 'mid-hide is not a bar height').toBeNull();
  });

  it('tolerates offsetHeight rounding up past the rect it is compared with', () => {
    // `offsetHeight` is an integer, so a 43.6px header at a fractional root
    // reports 44 and would fail a strict comparison while sitting perfectly
    // still. Rejecting that would leave a narrow packaged window's lights on
    // the last value it managed to push, for a header that never moved.
    mountHeader(43.6, 44);
    expect(measureHeaderBarHeight()).toBe(43.6);
    // The slack is a pixel, not a licence: a translate is tens of pixels.
    mountHeader(41, 44);
    expect(measureHeaderBarHeight()).toBeNull();
  });

  it('accepts the desktop shape, where the strip puts the bottom past the height', () => {
    // 28px band above a 20px header: the bottom is always strictly greater than
    // the header's own height here, so the transform guard can never misfire on
    // the layout the reserve actually exists in.
    mountHeader(48, 20);
    expect(measureHeaderBarHeight()).toBe(48);
  });
});

describe('pushTrafficLightOffset', () => {
  it('pushes the measured bar on boot', () => {
    pushTrafficLightOffset();
    expect(setTrafficLightOffsetMock).toHaveBeenCalledWith(48);
  });

  it('pushes again when the UI scale moves the bar', () => {
    // The reason this is a command rather than a value fixed at window build
    // time: the bar is 48px at 100% and 72px at 150%, and UI scale is live.
    pushTrafficLightOffset();
    mountHeader(72);
    pushTrafficLightOffset();
    expect(setTrafficLightOffsetMock.mock.calls).toEqual([[48], [72]]);
  });

  it('says nothing when the bar has not moved', () => {
    // `applyUiScale` runs on every preferences load, most of which change
    // nothing, so a re-measure that agrees must not spend an IPC round trip.
    pushTrafficLightOffset();
    pushTrafficLightOffset();
    expect(setTrafficLightOffsetMock).toHaveBeenCalledTimes(1);
  });

  it('does nothing off the packaged macOS build', () => {
    // `data-titlebar-overlay` is stamped pre-paint by `titlebar_inset_script`
    // and exists nowhere else, so it is the one signal that means "this window
    // has native lights". Without it there is nothing to place, and the command
    // is not registered as a no-op the frontend may lean on.
    overlayBuild = false;
    pushTrafficLightOffset();
    expect(setTrafficLightOffsetMock).not.toHaveBeenCalled();
  });

  it('does nothing in a browser, which has no shell to tell', () => {
    platformMocks.isTauri = false;
    pushTrafficLightOffset();
    expect(setTrafficLightOffsetMock).not.toHaveBeenCalled();
  });

  it('does nothing when there is no header to measure', () => {
    mountHeader(null);
    pushTrafficLightOffset();
    expect(setTrafficLightOffsetMock).not.toHaveBeenCalled();
  });

  it('swallows a rejection rather than surfacing an invisible cosmetic miss', async () => {
    // Best-effort telemetry carve-out: nothing here is user-initiated, and the
    // next scale or style apply re-pushes. An unhandled rejection would be the
    // real bug.
    setTrafficLightOffsetMock.mockImplementationOnce(() => Promise.reject(new Error('nope')));
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    expect(() => pushTrafficLightOffset()).not.toThrow();
    await Promise.resolve();
    expect(warn).toHaveBeenCalled();
    warn.mockRestore();
  });

  it('retries a bar height whose push FAILED, rather than remembering it', async () => {
    // The de-duplication is what makes the self-healing claim above true or
    // false. Recording a failed push as done would skip every later apply that
    // measures the same bar, which is most of them, and strand the lights at
    // whatever the shell last managed to apply for the rest of the session.
    setTrafficLightOffsetMock.mockImplementationOnce(() => Promise.reject(new Error('nope')));
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    pushTrafficLightOffset();
    await Promise.resolve();
    pushTrafficLightOffset();
    expect(setTrafficLightOffsetMock.mock.calls).toEqual([[48], [48]]);
    warn.mockRestore();
  });
});

describe('watchTitlebarBand', () => {
  // The contract the call sites used to be. A hand-written list of the applies
  // that move the bar is one somebody has to keep complete, and the cost of
  // missing one is silent: the shell keeps the stale height for the rest of the
  // session, because nothing else ever measures. A packaged window was reported
  // centring its lights 17pt down a 60pt bar.
  let observed: Element[] = [];
  let fireResize: () => void = () => {};
  let disconnected = 0;

  class FakeResizeObserver {
    constructor(private readonly cb: () => void) {}
    observe(el: Element): void {
      observed.push(el);
      // A real one delivers the element's current size straight away, which is
      // what makes this the boot push as well.
      fireResize = () => this.cb();
      this.cb();
    }
    disconnect(): void { disconnected++; }
    unobserve(): void {}
  }

  /** Every watch this describe starts, so none of them outlives its test. The
   *  window listener is global, so a leaked one answers the NEXT test's
   *  resize. */
  let stops: Array<() => void> = [];
  const watch = (): (() => void) => {
    const stop = watchTitlebarBand();
    stops.push(stop);
    return stop;
  };

  beforeEach(() => {
    observed = [];
    disconnected = 0;
    stops = [];
    (globalThis as { ResizeObserver?: unknown }).ResizeObserver = FakeResizeObserver;
  });

  afterEach(() => {
    for (const stop of stops) stop();
  });

  it('pushes the band it is watching as soon as it is observed', () => {
    watch();
    expect(observed).toHaveLength(1);
    expect(setTrafficLightOffsetMock).toHaveBeenCalledWith(48);
  });

  it('pushes again when the band itself changes size', () => {
    // A UI scale or a retuned --desktop-bar-height, without either writer having
    // to remember to say so.
    watch();
    mountHeader(60);
    fireResize();
    expect(setTrafficLightOffsetMock.mock.calls).toEqual([[48], [60]]);
  });

  it('pushes on a window resize, which is what moves the band without resizing it', () => {
    // Crossing the mobile breakpoint rebuilds the bar out of different parts:
    // the strip is a flow sibling above the header on desktop, and the header
    // covers it on mobile. The band's own box can come out the same size while
    // its bottom moves.
    watch();
    mountHeader(44, 44);
    window.dispatchEvent(new Event('resize'));
    expect(setTrafficLightOffsetMock.mock.calls).toEqual([[48], [44]]);
  });

  it('stops watching when the surface unmounts', () => {
    watch()();
    expect(disconnected).toBe(1);
    mountHeader(60);
    window.dispatchEvent(new Event('resize'));
    expect(setTrafficLightOffsetMock.mock.calls).toEqual([[48]]);
  });

  it('pushes nothing on a build with no native lights', () => {
    overlayBuild = false;
    watch();
    expect(setTrafficLightOffsetMock).not.toHaveBeenCalled();
  });

  it('still watches when the lights attribute has not landed yet', () => {
    // `data-titlebar-overlay` arrives by an injected eval, and the effect that
    // starts this runs once. Declining the watch for a missing attribute would
    // decline it for the life of the page, where declining one push costs one
    // observation. The window IS a packaged one; only the stamp is late.
    overlayBuild = false;
    watch();
    expect(observed, 'the band is watched anyway').toHaveLength(1);
    overlayBuild = true;
    fireResize();
    expect(setTrafficLightOffsetMock).toHaveBeenCalledWith(48);
  });

  it('observes nothing in a browser, which has no shell to tell', () => {
    platformMocks.isTauri = false;
    watch();
    expect(observed).toHaveLength(0);
    expect(setTrafficLightOffsetMock).not.toHaveBeenCalled();
  });

  it('observes nothing when the surface declares no band', () => {
    mountHeader(null);
    watch();
    expect(observed).toHaveLength(0);
  });
});

describe('every surface with native lights on it owns its own band', () => {
  // Source scans, because what makes the placement right is that the surface
  // STATES its bar and WATCHES it, rather than inheriting either. The two are
  // mutually exclusive renders, so neither can cover the other. The picker
  // mounts no app shell: keyed on `.app-header`, the measurement found nothing
  // there, so it pushed nothing and wore whichever bar the last app page had
  // persisted. No preferences load reaches it either.
  const here: string = dirname(fileURLToPath(import.meta.url));
  const read = (path: string): string =>
    readFileSync(resolve(here, '../../components', path), 'utf-8');

  for (const [what, path] of Object.entries({
    'the app shell header': 'layout/AppHeader.tsx',
    'the workspace picker': 'picker/WorkspacePicker.tsx',
  })) {
    it(`${what} carries data-titlebar-band`, () => {
      expect(read(path)).toContain('data-titlebar-band');
    });

    it(`${what} calls watchTitlebarBand`, () => {
      expect(read(path)).toContain('watchTitlebarBand()');
    });
  }

  it('no preference apply hand-pushes beside them', () => {
    // The drift this replaced: `applyUiScale` and `applyStyleOverrides` each
    // carried a push, and between them they still missed every other mover.
    // Two mechanisms would also disagree about which measurement is live, since
    // only the observer runs after layout.
    const prefs: string = readFileSync(resolve(here, 'preferences.ts'), 'utf-8');
    expect(prefs).not.toContain('pushTrafficLightOffset()');
  });
});
