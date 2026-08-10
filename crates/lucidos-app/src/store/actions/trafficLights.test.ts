/**
 * The one number the shell cannot compute for itself.
 *
 * The macOS traffic lights are centred on our header bar by
 * `src/traffic_lights.rs`, but the bar's height is `--titlebar-inset` plus a
 * rem-authored `--app-header-height`, so it depends on the user's UI scale and
 * exists only in the page. These scans pin the three properties that make the
 * push correct: it happens on boot and on every scale change, it measures the
 * rendered header rather than restating `3rem`, and it does not fire at all on a
 * build with no native lights.
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
    selector === '.app-header' && headerBottom !== null
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

  it('answers null when there is no header mounted to measure', () => {
    // The workspace picker is its own full-window surface with no .app-header,
    // and a push with nothing measured must not invent a bar.
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

describe('the push is wired into every apply that can move the bar', () => {
  // A source scan, because the call sites are what the contract is: the bar
  // moves when the root font size does, and BOTH paths that write it have to
  // tell the shell. `applyUiScale` covers boot (loadPreferences calls it) and
  // the scale picker; `applyStyleOverrides` covers the Style Remote retuning
  // --user-ui-scale or the tokens the bar is built from over SSE. Each already
  // calls `clampThreadDrawerWidth()` for exactly the same reason, which is why
  // missing one is easy and invisible until someone changes their scale.
  const here: string = dirname(fileURLToPath(import.meta.url));
  const prefs: string = readFileSync(resolve(here, 'preferences.ts'), 'utf-8');

  const bodyOf = (name: string): string => {
    const at = prefs.indexOf(`export function ${name}(`);
    expect(at, `${name} not found in preferences.ts`).toBeGreaterThanOrEqual(0);
    const open = prefs.indexOf('{', at);
    let depth = 0;
    for (let i = open; i < prefs.length; i++) {
      if (prefs[i] === '{') depth++;
      else if (prefs[i] === '}' && --depth === 0) return prefs.slice(open + 1, i);
    }
    throw new Error(`unterminated body for ${name}`);
  };

  for (const fn of ['applyUiScale', 'applyStyleOverrides']) {
    it(`${fn} pushes the new bar height`, () => {
      expect(bodyOf(fn)).toContain('pushTrafficLightOffset()');
    });
  }

  it('applyUiScale pushes AFTER re-asserting the style overrides', () => {
    // The push MEASURES the rendered header, so it has to run against the bar
    // the user will actually see. `applyUiScale` writes the preference scale
    // inline and then `reapplyStyleOverrides()` puts a remote override of
    // --user-ui-scale back on top, so measuring in between centres the lights
    // for a bar that never paints, and nothing measures again until the scale
    // changes once more. The two measurements above it stay BEFORE the
    // re-assert on purpose (the gutter they publish is the one the transcript
    // actually reserved), which is why this is an ordering check on one call
    // and not on the block.
    const body = bodyOf('applyUiScale');
    expect(body.indexOf('pushTrafficLightOffset()'))
      .toBeGreaterThan(body.indexOf('reapplyStyleOverrides()'));
  });
});
