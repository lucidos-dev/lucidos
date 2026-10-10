/**
 * A plain toast raised again while its twin is up bumps a counter on the card
 * instead of stacking a copy. Otherwise nine lazy chunks failing at once would
 * cover the screen with nine identical "Failed to load" cards.
 *
 * Only a toast with nothing to act on merges. Two cards with the same words but
 * their own action or click handler may do different things.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { focusedPane, showToast, toasts, TOAST_AUTO_DISMISS_MS } from '../store';
import { ToastList } from '../../components/shared/Toast';
import { findByClass, textOf } from '../../components/layout/__tests__/vnodeWalk';

describe('a repeated plain toast counts instead of stacking', () => {
  beforeEach(() => {
    toasts.value = [];
    focusedPane.value = 'thread';
  });
  afterEach(() => { vi.useRealTimers(); });

  it('merges identical errors into one card with a count', () => {
    for (let i = 0; i < 9; i++) showToast('Failed to load. Refresh the page to try again.', 'error');

    expect(toasts.value).toHaveLength(1);
    expect(toasts.value[0].count).toBe(9);
  });

  it('draws the count on the card, and no counter on a single toast', () => {
    showToast('Saved', 'success');
    expect(toasts.value[0].count).toBeUndefined();
    expect(findByClass(ToastList(), 'toast-count')).toHaveLength(0);

    showToast('Saved', 'success');
    showToast('Saved', 'success');
    const [counter] = findByClass(ToastList(), 'toast-count');
    expect(textOf(counter)).toBe('×3');
  });

  it('keeps toasts apart when the type, title or message differs', () => {
    showToast('Failed', 'error');
    showToast('Failed', 'warning');
    showToast('Failed', 'error', { title: 'Upload' });
    showToast('Failed again', 'error');

    expect(toasts.value).toHaveLength(4);
  });

  it('keeps a repeat over the other pane on a card of its own', () => {
    focusedPane.value = 'thread';
    showToast('Failed', 'error');
    focusedPane.value = 'content';
    showToast('Failed', 'error');

    expect(toasts.value.map((t) => t.pane)).toEqual(['content', 'thread']);
  });

  it('never merges a toast that acts', () => {
    const onClick = () => {};
    showToast('Open it', 'info', { onClick });
    showToast('Open it', 'info', { onClick });
    showToast('Retry?', 'error', { action: { label: 'Retry', onClick } });
    showToast('Retry?', 'error', { action: { label: 'Retry', onClick } });

    expect(toasts.value).toHaveLength(4);
  });

  it('takes the lifetime of the latest repeat', () => {
    vi.useFakeTimers();
    showToast('Sync failed', 'error', { autoDismissMs: 1_000 });
    expect(toasts.value[0].persistent).toBe(false);
    showToast('Sync failed', 'error');
    vi.advanceTimersByTime(2_000);

    expect(toasts.value).toHaveLength(1);
    expect(toasts.value[0].persistent).toBe(true);
  });

  it('restarts the timer of a timed toast on each repeat', () => {
    vi.useFakeTimers();
    showToast('Copied', 'info');
    vi.advanceTimersByTime(TOAST_AUTO_DISMISS_MS - 100);
    showToast('Copied', 'info');
    vi.advanceTimersByTime(200);

    expect(toasts.value).toHaveLength(1);
    expect(toasts.value[0].count).toBe(2);

    vi.advanceTimersByTime(TOAST_AUTO_DISMISS_MS);
    expect(toasts.value).toHaveLength(0);
  });
});
