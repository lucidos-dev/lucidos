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

let headerBottom: number | null = 48;
let overlayBuild = true;

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
    mountHeader(72);
    expect(measureHeaderBarHeight()).toBe(72);
  });

  it('answers null when no surface has declared a band to measure', () => {
    mountHeader(null);
    expect(measureHeaderBarHeight()).toBeNull();
  });

  it('answers null for a header that has not been laid out', () => {
    mountHeader(0);
    expect(measureHeaderBarHeight()).toBeNull();
  });

  it('answers null for a header translated away by hide-on-scroll', () => {
    mountHeader(44, 44);
    expect(measureHeaderBarHeight(), 'at rest the whole header IS the bar').toBe(44);
    mountHeader(19, 44);
    expect(measureHeaderBarHeight(), 'mid-hide is not a bar height').toBeNull();
  });

  it('tolerates offsetHeight rounding up past the rect it is compared with', () => {
    mountHeader(43.6, 44);
    expect(measureHeaderBarHeight()).toBe(43.6);
    mountHeader(41, 44);
    expect(measureHeaderBarHeight()).toBeNull();
  });

  it('accepts the desktop shape, where the strip puts the bottom past the height', () => {
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
    pushTrafficLightOffset();
    mountHeader(72);
    pushTrafficLightOffset();
    expect(setTrafficLightOffsetMock.mock.calls).toEqual([[48], [72]]);
  });

  it('says nothing when the bar has not moved', () => {
    pushTrafficLightOffset();
    pushTrafficLightOffset();
    expect(setTrafficLightOffsetMock).toHaveBeenCalledTimes(1);
  });

  it('does nothing off the packaged macOS build', () => {
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
    setTrafficLightOffsetMock.mockImplementationOnce(() => Promise.reject(new Error('nope')));
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    expect(() => pushTrafficLightOffset()).not.toThrow();
    await Promise.resolve();
    expect(warn).toHaveBeenCalled();
    warn.mockRestore();
  });

  it('retries a bar height whose push FAILED, rather than remembering it', async () => {
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
  let observed: Element[] = [];
  let fireResize: () => void = () => {};
  let disconnected = 0;

  class FakeResizeObserver {
    constructor(private readonly cb: () => void) {}
    observe(el: Element): void {
      observed.push(el);
      fireResize = () => this.cb();
      this.cb();
    }
    disconnect(): void { disconnected++; }
    unobserve(): void {}
  }

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
    watch();
    mountHeader(60);
    fireResize();
    expect(setTrafficLightOffsetMock.mock.calls).toEqual([[48], [60]]);
  });

  it('pushes on a window resize, which is what moves the band without resizing it', () => {
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
    const prefs: string = readFileSync(resolve(here, 'preferences.ts'), 'utf-8');
    expect(prefs).not.toContain('pushTrafficLightOffset()');
  });
});
