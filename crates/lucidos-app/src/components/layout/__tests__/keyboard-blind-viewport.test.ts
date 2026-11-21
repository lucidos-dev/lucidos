// @vitest-environment jsdom
/**
 * A blind visual viewport still keeps the composer above the keyboard.
 *
 * Replays a phone's own log. The PWA opened from a notification tap with a
 * visual viewport of height 0. It then read 874, the full screen, and stayed
 * there with the keyboard up. The shell kept the full height, so the composer
 * sat under the keys until a relaunch.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { trackAppHeight, KEYBOARD_BAND_STORAGE_KEY } from '../MobileSwipeContainer';
import { resetKeyboardCloseState } from '../keyboardCloseRelayout';

const PORTRAIT = { width: 402, height: 874 };
const KEYS_UP = 478;

let viewport: EventTarget & { height: number; offsetTop: number };
let teardown: (() => void) | undefined;

const appHeight = () => document.documentElement.style.getPropertyValue('--app-height');
const band = () => document.documentElement.style.getPropertyValue('--keyboard-band');
const resizeViewport = (height: number) => {
  viewport.height = height;
  viewport.dispatchEvent(new Event('resize'));
};
const mountField = () => {
  const field = document.createElement('textarea');
  document.body.appendChild(field);
  return field;
};
const launch = (vvHeight: number) => {
  viewport.height = vvHeight;
  teardown = trackAppHeight();
};

describe('trackAppHeight with a blind visual viewport', () => {
  beforeEach(() => {
    vi.stubGlobal('scrollTo', () => {});
    localStorage.clear();
    viewport = Object.assign(new EventTarget(), { height: PORTRAIT.height, offsetTop: 0 });
    Object.defineProperty(window, 'visualViewport', { value: viewport, configurable: true });
    Object.defineProperty(window, 'innerWidth', { value: PORTRAIT.width, configurable: true });
    Object.defineProperty(window, 'innerHeight', { value: PORTRAIT.height, configurable: true });
  });

  afterEach(() => {
    teardown?.();
    teardown = undefined;
    resetKeyboardCloseState();
    document.body.innerHTML = '';
    localStorage.clear();
    vi.unstubAllGlobals();
  });

  it('shrinks the shell by the remembered keyboard on focus, after a zero-height launch', () => {
    localStorage.setItem(KEYBOARD_BAND_STORAGE_KEY, JSON.stringify({ portrait: PORTRAIT.height - KEYS_UP }));
    launch(0);
    viewport.height = PORTRAIT.height;

    mountField().focus();
    expect(appHeight()).toBe(`${KEYS_UP}px`);
    expect(band()).toBe(`${PORTRAIT.height - KEYS_UP}px`);
  });

  it('falls back to the assumed keyboard when none was ever measured', () => {
    launch(0);
    mountField().focus();
    expect(appHeight()).toBe(`${PORTRAIT.height - Math.round(PORTRAIT.height * 0.45)}px`);
  });

  it('gives the full height back when the field lets go', () => {
    launch(0);
    const field = mountField();
    field.focus();
    field.blur();
    expect(appHeight()).toBe(`${PORTRAIT.height}px`);
    expect(band()).toBe('0px');
  });

  it('trusts the viewport again once it measures a keyboard', () => {
    launch(0);
    const field = mountField();
    field.focus();
    resizeViewport(500);
    expect(appHeight()).toBe('500px');

    field.blur();
    resizeViewport(PORTRAIT.height);
    field.focus();
    // Seeing again: the height waits for the viewport's own reading.
    expect(appHeight()).toBe(`${PORTRAIT.height}px`);
  });

  it('remembers a measured keyboard across launches', () => {
    launch(PORTRAIT.height);
    const field = mountField();
    field.focus();
    resizeViewport(KEYS_UP);
    field.blur();
    resizeViewport(PORTRAIT.height);
    teardown?.();
    document.body.innerHTML = '';

    launch(0);
    mountField().focus();
    expect(appHeight()).toBe(`${KEYS_UP}px`);
  });

  it('goes blind on a zero that follows real readings, before any keyboard was measured', () => {
    // A phone's log: the launch read 874, then 812, then 0, and the keys up
    // later read 0, the full screen, or a large negative.
    localStorage.setItem(KEYBOARD_BAND_STORAGE_KEY, JSON.stringify({ portrait: PORTRAIT.height - KEYS_UP }));
    launch(PORTRAIT.height);
    resizeViewport(812);
    resizeViewport(0);
    viewport.height = PORTRAIT.height;

    mountField().focus();
    expect(appHeight()).toBe(`${KEYS_UP}px`);
  });

  it('goes blind on a negative reading', () => {
    launch(PORTRAIT.height);
    resizeViewport(-33554432);

    mountField().focus();
    expect(appHeight()).toBe(`${PORTRAIT.height - Math.round(PORTRAIT.height * 0.45)}px`);
  });

  it('takes a wake zero after a measured keyboard as transient, not blind', () => {
    launch(PORTRAIT.height);
    const field = mountField();
    field.focus();
    resizeViewport(KEYS_UP);
    field.blur();
    resizeViewport(PORTRAIT.height);
    viewport.height = 0;
    document.dispatchEvent(new Event('visibilitychange'));
    viewport.height = PORTRAIT.height;

    mountField().focus();
    expect(appHeight()).toBe(`${PORTRAIT.height}px`);
  });

  it('blurs a focus iOS kept across a wake while blind', () => {
    launch(0);
    const field = mountField();
    field.focus();
    viewport.height = PORTRAIT.height;

    document.dispatchEvent(new Event('visibilitychange'));
    expect(document.activeElement).not.toBe(field);
    expect(appHeight()).toBe(`${PORTRAIT.height}px`);
  });

  it('leaves a seeing viewport to its own readings', () => {
    launch(PORTRAIT.height);
    mountField().focus();
    expect(appHeight()).toBe(`${PORTRAIT.height}px`);
    resizeViewport(KEYS_UP);
    expect(appHeight()).toBe(`${KEYS_UP}px`);
  });
});
