// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { USER_SCROLL_WINDOW_MS, isUserScrolling } from './scrollActivity';

function press(target: Element, key: string, init: KeyboardEventInit = {}) {
  target.dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true, ...init }));
}

let clock = 10_000_000;

describe('isUserScrolling', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(clock);
    vi.spyOn(performance, 'now').mockImplementation(() => Date.now());
  });
  afterEach(() => {
    clock += 10 * USER_SCROLL_WINDOW_MS;
    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  it('is quiet with no input', () => {
    expect(isUserScrolling()).toBe(false);
  });

  it('counts a Home key on the transcript, which WebKit animates', () => {
    const region = document.createElement('div');
    region.tabIndex = 0;
    document.body.append(region);
    press(region, 'Home');
    expect(isUserScrolling()).toBe(true);
    vi.advanceTimersByTime(USER_SCROLL_WINDOW_MS + 1);
    expect(isUserScrolling()).toBe(false);
  });

  it('counts a wheel notch', () => {
    document.body.dispatchEvent(new WheelEvent('wheel', { bubbles: true }));
    expect(isUserScrolling()).toBe(true);
  });

  it('ignores typing in a text field, a chord, and a key that scrolls nothing', () => {
    const input = document.createElement('textarea');
    document.body.append(input);
    press(input, ' ');
    press(input, 'End');
    press(document.body, 'ArrowUp', { metaKey: true });
    press(document.body, 'a');
    expect(isUserScrolling()).toBe(false);
  });
});
