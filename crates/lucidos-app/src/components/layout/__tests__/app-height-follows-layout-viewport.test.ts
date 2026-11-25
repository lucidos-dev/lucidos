// @vitest-environment jsdom
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { enginePackaged } from '../../../store/enginePackaging';
import { PERF_FLAG_KEY, clearPerfOptIn, perfRecordingOn, setPerfEnabled } from '../../../utils/perfQueue';
import { trackAppHeight } from '../MobileSwipeContainer';
import { resetKeyboardCloseState } from '../keyboardCloseRelayout';

const LANDSCAPE = { width: 874, height: 402 };
const PORTRAIT = { width: 402, height: 874 };

let viewport: EventTarget & { height: number; offsetTop: number };
let teardown: (() => void) | undefined;

const setLayoutViewport = (v: { width: number; height: number }) => {
  Object.defineProperty(window, 'innerWidth', { value: v.width, configurable: true });
  Object.defineProperty(window, 'innerHeight', { value: v.height, configurable: true });
};
const appHeight = () => document.documentElement.style.getPropertyValue('--app-height');
const wake = () => document.dispatchEvent(new Event('visibilitychange'));

describe('trackAppHeight', () => {
  beforeEach(() => {
    vi.stubGlobal('scrollTo', () => {});
    viewport = Object.assign(new EventTarget(), { height: LANDSCAPE.height, offsetTop: 0 });
    Object.defineProperty(window, 'visualViewport', { value: viewport, configurable: true });
    setLayoutViewport(LANDSCAPE);
    teardown = trackAppHeight();
  });

  afterEach(() => {
    teardown?.();
    resetKeyboardCloseState();
    document.body.innerHTML = '';
    vi.unstubAllGlobals();
  });

  it('writes the corrected height when iOS fixes a stale wake reading with a window resize', () => {
    expect(appHeight()).toBe('402px');

    setLayoutViewport({ width: PORTRAIT.width, height: LANDSCAPE.height });
    viewport.height = 0;
    wake();
    expect(appHeight()).toBe('402px');

    setLayoutViewport(PORTRAIT);
    window.dispatchEvent(new Event('resize'));
    expect(appHeight()).toBe('874px');
  });

  it('keeps the keyboard-up height through a window resize', () => {
    setLayoutViewport(PORTRAIT);
    viewport.height = 478;
    const field = document.createElement('textarea');
    document.body.appendChild(field);
    field.focus();

    window.dispatchEvent(new Event('resize'));
    expect(appHeight()).toBe('478px');
  });

  it('keeps the focus reserve through a window resize that moved nothing', () => {
    const band = () => document.documentElement.style.getPropertyValue('--keyboard-band');
    const field = document.createElement('textarea');
    document.body.appendChild(field);
    field.focus();
    const reserved = band();
    expect(reserved).not.toBe('0px');

    window.dispatchEvent(new Event('resize'));
    expect(band()).toBe(reserved);
  });

  it('starts from-source sampling without polling until the device opts in', () => {
    const interval = vi.spyOn(globalThis, 'setInterval');
    const clear = vi.spyOn(globalThis, 'clearInterval');
    try {
      expect(perfRecordingOn()).toBe(false);
      enginePackaged.value = false;
      expect(perfRecordingOn()).toBe(true);
      expect(interval).not.toHaveBeenCalled();

      localStorage.setItem(PERF_FLAG_KEY, '1');
      window.dispatchEvent(Object.assign(new Event('storage'), {
        key: PERF_FLAG_KEY, newValue: '1', storageArea: window.localStorage,
      }));
      expect(interval).toHaveBeenCalledExactlyOnceWith(expect.any(Function), 3000);

      localStorage.removeItem(PERF_FLAG_KEY);
      window.dispatchEvent(Object.assign(new Event('storage'), {
        key: PERF_FLAG_KEY, oldValue: '1', storageArea: window.localStorage,
      }));
      expect(clear).toHaveBeenCalledExactlyOnceWith(interval.mock.results[0].value);
      expect(perfRecordingOn()).toBe(true);
    } finally {
      enginePackaged.value = null;
      localStorage.removeItem(PERF_FLAG_KEY);
      interval.mockRestore();
      clear.mockRestore();
    }
  });

  it('stops polling when a from-source device clears its old opt-in in the same tab', () => {
    const interval = vi.spyOn(globalThis, 'setInterval');
    const clear = vi.spyOn(globalThis, 'clearInterval');
    try {
      enginePackaged.value = false;
      localStorage.setItem(PERF_FLAG_KEY, '1');
      window.dispatchEvent(Object.assign(new Event('storage'), {
        key: PERF_FLAG_KEY, newValue: '1', storageArea: window.localStorage,
      }));
      expect(interval).toHaveBeenCalledExactlyOnceWith(expect.any(Function), 3000);

      clearPerfOptIn();

      expect(clear).toHaveBeenCalledExactlyOnceWith(interval.mock.results[0].value);
      expect(localStorage.getItem(PERF_FLAG_KEY)).toBeNull();
      expect(perfRecordingOn()).toBe(true);
    } finally {
      enginePackaged.value = null;
      localStorage.removeItem(PERF_FLAG_KEY);
      interval.mockRestore();
      clear.mockRestore();
    }
  });

  it('starts and stops polling from the packaged switch in the same tab', () => {
    const interval = vi.spyOn(globalThis, 'setInterval');
    const clear = vi.spyOn(globalThis, 'clearInterval');
    try {
      enginePackaged.value = true;
      setPerfEnabled(true);
      expect(interval).toHaveBeenCalledExactlyOnceWith(expect.any(Function), 3000);
      setPerfEnabled(false);
      expect(clear).toHaveBeenCalledExactlyOnceWith(interval.mock.results[0].value);
    } finally {
      setPerfEnabled(false);
      enginePackaged.value = null;
      interval.mockRestore();
      clear.mockRestore();
    }
  });

  it('stops listening on teardown', () => {
    teardown?.();
    teardown = undefined;
    setLayoutViewport(PORTRAIT);
    window.dispatchEvent(new Event('resize'));
    expect(appHeight()).toBe('');
  });
});
