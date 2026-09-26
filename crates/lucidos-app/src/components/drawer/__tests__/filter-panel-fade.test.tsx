// @vitest-environment jsdom
/**
 * The thread filter panel's swap with the thread list (`ThreadFilterCover`).
 *
 * The cover and the panel inside it stay mounted, so opening costs no render.
 * The open SIGNAL drives everything at once: the overlay stack, the pressed
 * Filter button, and the cover's `data-open` and `inert`. Each swap mounts a
 * fresh navigation cover, the same one a content-pane navigation gets.
 *
 * jsdom runs no animations, so these tests pin the DOM states the CSS keys on.
 * The frames themselves are covered by
 * `e2e/threads-header-filter-transitions.spec.ts`.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { ThreadFilterCover } from '../ThreadFilterCover';
import { threadFilterPanelOpen, openThreadFilterPanel, closeThreadFilterPanel } from '../../../store/threadFilterPanel';
import { overlayStack } from '../../../store/overlayStack';
import { filterButtonState } from '../../layout/ThreadFilterPanel';

describe('the filter panel swaps under a navigation cover, and the signal leads', () => {
  let host: HTMLElement;
  const cover = () => host.querySelector('.thread-filter-cover') as HTMLElement;
  const panel = () => host.querySelector('.thread-filter-panel');
  const navCover = () => host.querySelector('.nav-cover');
  const onStack = () => overlayStack.value.some(e => e.id === 'thread-filter-panel');
  const pressed = () => filterButtonState({
    view: 'all', panelOpen: threadFilterPanelOpen.value, channelFilterActive: false, attentionCount: 0,
  }).pressed;

  beforeEach(() => {
    closeThreadFilterPanel();
    host = document.createElement('div');
    document.body.appendChild(host);
    act(() => { render(<ThreadFilterCover paneVisible />, host); });
  });

  afterEach(() => {
    act(() => { render(null, host); });
    host.remove();
    closeThreadFilterPanel();
  });

  it('keeps the panel mounted under an inert cover while shut', () => {
    expect(cover().hasAttribute('data-open')).toBe(false);
    expect(cover().hasAttribute('inert')).toBe(true);
    expect(panel()).not.toBeNull();
    expect(panel()!.parentElement).toBe(cover());
  });

  it('opens on the same panel, with no remount', () => {
    const shut = panel();
    act(() => { openThreadFilterPanel(); });
    expect(cover().hasAttribute('data-open')).toBe(true);
    expect(cover().hasAttribute('inert')).toBe(false);
    expect(panel()).toBe(shut);
  });

  it('closes at once: the stack entry, the pressed state and the cover', () => {
    act(() => { openThreadFilterPanel(); });
    const open = panel();
    act(() => { closeThreadFilterPanel(); });
    expect(onStack()).toBe(false);
    expect(pressed()).toBe(false);
    expect(cover().hasAttribute('data-open')).toBe(false);
    // A leaving cover takes no pointer and no focus.
    expect(cover().hasAttribute('inert')).toBe(true);
    expect(panel()).toBe(open);
  });

  it('reopens straight away on the same panel', () => {
    act(() => { openThreadFilterPanel(); });
    const first = panel();
    act(() => { closeThreadFilterPanel(); });
    act(() => { openThreadFilterPanel(); });
    expect(panel()).toBe(first);
    expect(cover().hasAttribute('data-open')).toBe(true);
    expect(onStack()).toBe(true);
  });

  it('mounts no navigation cover on the first render, so a restored panel shows at once', () => {
    expect(navCover()).toBeNull();
  });

  it('covers each swap with a fresh cover, over the filter cover', () => {
    act(() => { openThreadFilterPanel(); });
    const opening = navCover();
    expect(opening).not.toBeNull();
    // After the filter cover, so it paints over it at any equal z-index too.
    expect(cover().nextElementSibling).toBe(opening);
    expect(opening!.getAttribute('aria-hidden')).toBe('true');
    act(() => { closeThreadFilterPanel(); });
    // A new element, so its animation restarts from opaque.
    expect(navCover()).not.toBeNull();
    expect(navCover()).not.toBe(opening);
  });

  it('unmounts the cover on its fuse, animation or not', () => {
    vi.useFakeTimers();
    try {
      act(() => { openThreadFilterPanel(); });
      expect(navCover()).not.toBeNull();
      act(() => { vi.advanceTimersByTime(1_000); });
      expect(navCover()).toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });

  it('stays open but inert on a collapsed drawer, so its controls take no focus', () => {
    act(() => { openThreadFilterPanel(); });
    act(() => { render(<ThreadFilterCover paneVisible={false} />, host); });
    expect(cover().hasAttribute('data-open')).toBe(true);
    expect(cover().hasAttribute('inert')).toBe(true);
  });
});
