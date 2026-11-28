import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
import { ToastList } from '../Toast';
import { toasts, engineRestarting, dismissToast, showToast, TOAST_AUTO_DISMISS_MS } from '../../../store/store';
import { handleAppToastMessage } from '../../../store/actions/app-toast-bridge';
import { markSwUpdateDismissed } from '../../../hooks/sw-update';

vi.mock('../../../hooks/sw-update', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../hooks/sw-update')>()),
  markSwUpdateDismissed: vi.fn(),
  markEngineVersionDismissed: vi.fn(),
}));

function findByClass(node: ComponentChildren, cls: string, out: VNode[] = []): VNode[] {
  if (node === null || node === undefined || typeof node === 'boolean') return out;
  if (typeof node === 'string' || typeof node === 'number') return out;
  if (Array.isArray(node)) {
    for (const child of node) findByClass(child, cls, out);
    return out;
  }
  const v = node as VNode<{ class?: string; className?: string; children?: ComponentChildren }>;
  const classAttr = v.props?.class ?? v.props?.className ?? '';
  if (typeof classAttr === 'string' && classAttr.split(/\s+/).includes(cls)) {
    out.push(v);
  }
  findByClass(v.props?.children, cls, out);
  return out;
}

const toastMsg = (payload: Record<string, unknown>) =>
  handleAppToastMessage('lucidos:ui:toast', payload);
const dismissMsg = (payload: Record<string, unknown>) =>
  handleAppToastMessage('lucidos:ui:dismissToast', payload);

beforeEach(() => {
  toasts.value = [];
  engineRestarting.value = false;
  vi.mocked(markSwUpdateDismissed).mockClear();
});

describe('app toast bridge: spinning', () => {
  it('renders the spinner instead of the severity icon', () => {
    toastMsg({ message: 'Reindexing…', type: 'info', key: 'reindex', spinning: true });
    expect(toasts.value[0].spinning).toBe(true);
    expect(findByClass(ToastList(), 'mini-spinner').length).toBe(1);
  });

  it('renders the severity icon for an ordinary app toast', () => {
    toastMsg({ message: 'Saved', type: 'success' });
    expect(findByClass(ToastList(), 'mini-spinner').length).toBe(0);
    expect(findByClass(ToastList(), 'toast-icon').length).toBe(1);
  });

  it('ignores a non-boolean spinning rather than treating it as true', () => {
    toastMsg({ message: 'Reindexing…', type: 'info', spinning: 'yes' });
    expect(toasts.value[0].spinning).toBeUndefined();
    expect(findByClass(ToastList(), 'mini-spinner').length).toBe(0);
  });
});

describe('app toast bridge: title', () => {
  it('shows an app title as the bold line over the message', () => {
    toastMsg({ message: 'Nothing was built tonight.', type: 'info', title: 'Nightly skipped' });
    expect(toasts.value[0].title).toBe('Nightly skipped');
    expect(toasts.value[0].message).toBe('Nothing was built tonight.');
    expect(findByClass(ToastList(), 'toast-title').length).toBe(1);
  });

  it('ignores a title that is not a non-empty string', () => {
    toastMsg({ message: 'One', type: 'info', title: 42 });
    toastMsg({ message: 'Two', type: 'info', title: '' });
    expect(toasts.value.map((t) => t.title)).toEqual([undefined, undefined]);
  });
});

describe('app toast bridge: dismissToast', () => {
  it('removes the toast carrying that key', () => {
    toastMsg({ message: 'Reindexing…', type: 'info', key: 'reindex', spinning: true });
    expect(toasts.value.length).toBe(1);
    dismissMsg({ key: 'reindex' });
    expect(toasts.value.length).toBe(0);
  });

  it('leaves every other toast alone', () => {
    toastMsg({ message: 'Reindexing…', type: 'info', key: 'reindex' });
    toastMsg({ message: 'Syncing…', type: 'info', key: 'sync' });
    dismissMsg({ key: 'reindex' });
    expect(toasts.value.map((t) => t.key)).toEqual(['sync']);
  });

  it('is a silent no-op for a key matching nothing', () => {
    toastMsg({ message: 'Syncing…', type: 'info', key: 'sync' });
    const before = toasts.value;
    expect(() => dismissMsg({ key: 'no-such-toast' })).not.toThrow();
    expect(toasts.value).toBe(before);
  });

  it('is a no-op for a missing or non-string key', () => {
    toastMsg({ message: 'Syncing…', type: 'info', key: 'sync' });
    expect(() => dismissMsg({})).not.toThrow();
    expect(() => dismissMsg({ key: 42 })).not.toThrow();
    expect(toasts.value.length).toBe(1);
  });

  it('does not fire the host user-dismiss side effect for a host-owned key', () => {
    showToast('New version available', 'info', { key: 'update-available' });
    dismissMsg({ key: 'update-available' });
    expect(toasts.value.length).toBe(0);
    expect(markSwUpdateDismissed).not.toHaveBeenCalled();
  });

  it('but the host dismiss path still does', () => {
    showToast('New version available', 'info', { key: 'update-available' });
    dismissToast('update-available');
    expect(markSwUpdateDismissed).toHaveBeenCalledTimes(1);
  });
});

describe('app toast bridge: lifetime', () => {
  beforeEach(() => { vi.useFakeTimers(); });
  afterEach(() => { vi.useRealTimers(); });

  it('closes a keyed success toast on the host timer', () => {
    toastMsg({ message: 'Saved', type: 'success', key: 'save' });
    vi.advanceTimersByTime(TOAST_AUTO_DISMISS_MS);
    expect(toasts.value).toEqual([]);
  });

  it('closes an unkeyed info toast on the host timer', () => {
    toastMsg({ message: 'Synced', type: 'info' });
    vi.advanceTimersByTime(TOAST_AUTO_DISMISS_MS);
    expect(toasts.value).toEqual([]);
  });

  it('keeps errors, warnings and spinners up until dismissed or replaced', () => {
    toastMsg({ message: 'Failed', type: 'error', key: 'a' });
    toastMsg({ message: 'Careful', type: 'warning' });
    toastMsg({ message: 'Reindexing…', type: 'info', key: 'reindex', spinning: true });
    vi.advanceTimersByTime(TOAST_AUTO_DISMISS_MS * 10);
    expect(toasts.value.length).toBe(3);
  });

  it('closes an unkeyed spinner on the host timer', () => {
    toastMsg({ message: 'Working…', type: 'info', spinning: true, dismissable: false });
    vi.advanceTimersByTime(TOAST_AUTO_DISMISS_MS);
    expect(toasts.value).toEqual([]);
  });

  it('closes a spinner once its outcome replaces it', () => {
    toastMsg({ message: 'Reindexing…', type: 'info', key: 'reindex', spinning: true });
    toastMsg({ message: 'Reindexed', type: 'success', key: 'reindex' });
    vi.advanceTimersByTime(TOAST_AUTO_DISMISS_MS);
    expect(toasts.value).toEqual([]);
  });

  it.each(['success', 'info', 'warning', 'error'])('lets an app close a timed %s even with dismissable false', (type) => {
    toastMsg({ message: 'Finished', type, key: 'timed-app', durationMs: 8000, dismissable: false });
    const buttons = findByClass(ToastList(), 'toast-close');
    expect(buttons).toHaveLength(1);
    (buttons[0].props as unknown as { onClick: () => void }).onClick();
    expect(toasts.value).toEqual([]);
    expect(vi.getTimerCount()).toBe(0);
    toastMsg({ message: 'Still working', type: 'info', key: 'timed-app', spinning: true });
    vi.advanceTimersByTime(8000);
    expect(toasts.value).toHaveLength(1);
  });

  it('honours an explicit durationMs', () => {
    toastMsg({ message: 'Failed', type: 'error', key: 'a', durationMs: 1000 });
    vi.advanceTimersByTime(1000);
    expect(toasts.value).toEqual([]);
  });
});

describe('app toast bridge: message ownership', () => {
  it('claims both toast messages and no others', () => {
    expect(toastMsg({ message: 'x' })).toBe(true);
    expect(dismissMsg({ key: 'x' })).toBe(true);
    expect(handleAppToastMessage('lucidos:ui:confirm', { message: 'x' })).toBe(false);
    expect(handleAppToastMessage('lucidos:ui:prompt', { message: 'x' })).toBe(false);
    expect(handleAppToastMessage('lucidos:ui:preview-file', {})).toBe(false);
  });

  it('claims a message-less toast without showing anything', () => {
    expect(toastMsg({ type: 'info' })).toBe(true);
    expect(toasts.value.length).toBe(0);
  });
});
