// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { h, render } from 'preact';
import { act } from 'preact/test-utils';
import { useHideOnScroll } from './useHideOnScroll';
import { viewportIsMobile } from '../utils/viewport';
import { mobileView } from '../store/store';

describe('mobile header measurements after suspension', () => {
  let host: HTMLElement;
  const wasMobile = viewportIsMobile.peek();
  const previousView = mobileView.peek();

  afterEach(() => {
    if (host) {
      act(() => render(null, host));
      host.remove();
    }
    viewportIsMobile.value = wasMobile;
    mobileView.value = previousView;
    document.documentElement.style.removeProperty('--mobile-header-height');
    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  it.each(['pageshow', 'visibilitychange'])('remeasures on %s while holding the reader on their row', (event) => {
    vi.useFakeTimers();
    vi.spyOn(window, 'innerWidth', 'get').mockReturnValue(375);
    viewportIsMobile.value = true;
    mobileView.value = 'thread';
    host = document.createElement('div');
    document.body.append(host);
    const header = document.createElement('header');
    let height = 100;
    vi.spyOn(header, 'getBoundingClientRect').mockImplementation(() => ({ height }) as DOMRect);
    function Harness() {
      useHideOnScroll({ current: header });
      return h('div', { class: 'mobile-swipe-pane' },
        h('div', { class: 'thread-content visible' }));
    }
    act(() => render(h(Harness, {}), host));
    act(() => { vi.advanceTimersByTime(1000); });
    const scroller = host.querySelector('.thread-content')!;
    Object.defineProperty(scroller, 'scrollHeight', { value: 2000 });
    Object.defineProperty(scroller, 'clientHeight', { value: 500 });
    scroller.scrollTop = 300;
    const before = document.documentElement.style.getPropertyValue('--mobile-header-height');
    // No observer delivery or resize event while the browser is suspended.
    height = 60;
    act(() => {
      (event === 'pageshow' ? window : document).dispatchEvent(new Event(event));
    });
    const after = document.documentElement.style.getPropertyValue('--mobile-header-height');
    expect(after).not.toBe(before);
    const rem = parseFloat(getComputedStyle(document.documentElement).fontSize) || 16;
    expect(parseFloat(after) * rem).toBeCloseTo(height);
    expect(scroller.scrollTop).toBe(260);
  });
});
