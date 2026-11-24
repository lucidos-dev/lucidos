import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

import { classifyScroll, markAnchorScroll, markNavigationScroll } from '../scrollState';
import { WRITE_EVENT_WINDOW_MS } from '../scrollLedger';

describe('the page half of a scroll class', () => {
  const container = (scrollTop = 0) => ({ scrollTop, scrollHeight: 5000, clientHeight: 800 }) as HTMLElement;

  beforeEach(() => { vi.useFakeTimers(); });
  afterEach(() => { vi.useRealTimers(); });

  it('reads a write to another container as the page\'s navigation, not this container\'s', () => {
    const el = container(100);
    const other = container(0);
    vi.advanceTimersByTime(1000);
    markNavigationScroll(other, 40);
    const scroll = classifyScroll(el);
    expect(scroll.navigation).toBe(false);
    expect(scroll.kind).toBeNull();
    expect(scroll.page.navigation).toBe(true);
    expect(scroll.page.kind).toBe('placement');
  });

  it('names the kind of the page\'s LAST write, over this container\'s own', () => {
    const el = container(100);
    const other = container(0);
    vi.advanceTimersByTime(1000);
    markAnchorScroll(el, 120);
    markNavigationScroll(other, 40);
    const scroll = classifyScroll(el);
    expect(scroll.kind).toBe('anchor');
    expect(scroll.page.kind).toBe('placement');
  });

  it('ends with the event window of that write', () => {
    const el = container(100);
    const other = container(0);
    vi.advanceTimersByTime(1000);
    markNavigationScroll(other, 40);
    vi.advanceTimersByTime(WRITE_EVENT_WINDOW_MS + 1);
    expect(classifyScroll(el).page).toEqual({ navigation: false, kind: null });
  });
});
