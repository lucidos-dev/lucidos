// @vitest-environment jsdom
/**
 * The thread filter panel's fade through (`ThreadFilterCover`).
 *
 * The cover and its fade layer stay mounted, so the CSS fades run on elements
 * that already exist. The panel mounts on open and stays for the fade out.
 * The open SIGNAL still drives everything else at once: the overlay stack,
 * the pressed Filter button, and the cover's `inert`.
 *
 * jsdom runs no transitions, so these tests pin the DOM states the CSS keys on
 * (`data-open`, `inert`, the panel's presence). The frames themselves are
 * covered by `e2e/threads-header-filter-transitions.spec.ts`.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { ThreadFilterCover } from '../ThreadFilterCover';
import { threadFilterPanelOpen, openThreadFilterPanel, closeThreadFilterPanel } from '../../../store/threadFilterPanel';
import { overlayStack } from '../../../store/overlayStack';
import { filterButtonState } from '../../layout/ThreadFilterPanel';
import { motionPreference } from '../../../utils/motion';

/** Past the 150ms fade and its 50ms slack, at 1x. */
const PAST_EXIT_MS = 201;

describe('the filter panel fades through, and the signal still leads', () => {
  let host: HTMLElement;
  const cover = () => host.querySelector('.thread-filter-cover') as HTMLElement;
  const panel = () => host.querySelector('.thread-filter-panel');
  const onStack = () => overlayStack.value.some(e => e.id === 'thread-filter-panel');
  const pressed = () => filterButtonState({
    view: 'all', panelOpen: threadFilterPanelOpen.value, channelFilterActive: false, attentionCount: 0,
  }).pressed;

  beforeEach(() => {
    vi.useFakeTimers();
    closeThreadFilterPanel();
    host = document.createElement('div');
    document.body.appendChild(host);
    act(() => { render(<ThreadFilterCover />, host); });
  });

  afterEach(() => {
    act(() => { render(null, host); });
    host.remove();
    closeThreadFilterPanel();
    motionPreference.value = 'system';
    vi.useRealTimers();
  });

  it('keeps an empty, inert cover mounted while shut', () => {
    expect(cover()).not.toBeNull();
    expect(cover().hasAttribute('data-open')).toBe(false);
    expect(cover().hasAttribute('inert')).toBe(true);
    expect(cover().querySelector('.thread-filter-fade')).not.toBeNull();
    expect(panel()).toBeNull();
  });

  it('opens the cover and mounts the panel in the same render', () => {
    act(() => { openThreadFilterPanel(); });
    expect(cover().hasAttribute('data-open')).toBe(true);
    expect(cover().hasAttribute('inert')).toBe(false);
    expect(panel()).not.toBeNull();
  });

  it('keeps the panel for the fade out, while the signal has already moved on', () => {
    act(() => { openThreadFilterPanel(); });
    act(() => { closeThreadFilterPanel(); });
    // At once: the stack entry, the pressed state and the cover's state.
    expect(onStack()).toBe(false);
    expect(pressed()).toBe(false);
    expect(cover().hasAttribute('data-open')).toBe(false);
    // A leaving cover takes no pointer and no focus.
    expect(cover().hasAttribute('inert')).toBe(true);
    // But the options are still there to fade.
    expect(panel()).not.toBeNull();

    act(() => { vi.advanceTimersByTime(PAST_EXIT_MS); });
    expect(panel()).toBeNull();
  });

  it('reopens during the fade out on the same panel, with no remount', () => {
    act(() => { openThreadFilterPanel(); });
    const first = panel();
    act(() => { closeThreadFilterPanel(); });
    act(() => { vi.advanceTimersByTime(100); });
    act(() => { openThreadFilterPanel(); });
    expect(panel()).toBe(first);
    expect(cover().hasAttribute('data-open')).toBe(true);
    expect(onStack()).toBe(true);
    // The old exit timer must not unmount the reopened panel.
    act(() => { vi.advanceTimersByTime(PAST_EXIT_MS); });
    expect(panel()).toBe(first);
  });

  it('closes during the fade in straight into the fade out', () => {
    act(() => { openThreadFilterPanel(); });
    const first = panel();
    act(() => { closeThreadFilterPanel(); });
    expect(cover().hasAttribute('data-open')).toBe(false);
    expect(panel()).toBe(first);
  });

  it('does not keep the panel for a fade under reduced motion', () => {
    // Reduced motion scales the fade to next to nothing. Only the fixed slack
    // is left, and CSS has already hidden the cover for all of it.
    motionPreference.value = 'reduce';
    act(() => { openThreadFilterPanel(); });
    act(() => { closeThreadFilterPanel(); });
    act(() => { vi.advanceTimersByTime(51); });
    expect(panel()).toBeNull();
  });
});
