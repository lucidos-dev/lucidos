// @vitest-environment jsdom
/**
 * `--app-height` follows every correction iOS makes to the layout viewport.
 *
 * Replays a phone's own log. It went to the background in landscape and came
 * back in portrait. At the wake iOS reported the landscape height (402) at the
 * portrait width, with a visual viewport of 0. A window `resize` then brought
 * the height back to 874, and no visual-viewport resize followed. The shell
 * stayed 402 tall, under half the screen, until the keyboard next moved.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
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

  it('stops listening on teardown', () => {
    teardown?.();
    teardown = undefined;
    setLayoutViewport(PORTRAIT);
    window.dispatchEvent(new Event('resize'));
    expect(appHeight()).toBe('');
  });
});
