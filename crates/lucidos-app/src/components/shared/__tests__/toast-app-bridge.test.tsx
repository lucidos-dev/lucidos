import { describe, it, expect, beforeEach, vi } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
import { ToastList } from '../Toast';
import { toasts, engineRestarting, dismissToast, showToast } from '../../../store/store';
import { handleAppToastMessage } from '../../../store/actions/app-toast-bridge';
import { markSwUpdateDismissed } from '../../../hooks/sw-update';

// The store's keyed `dismissToast` layers user-dismiss side effects on top of the
// structural removal for two host-owned keys. Mocked so the tests below can prove
// the app bridge does NOT reach them, and (in the contrast case) that the host
// path still does, i.e. that the assertion isn't vacuous.
vi.mock('../../../hooks/sw-update', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../hooks/sw-update')>()),
  markSwUpdateDismissed: vi.fn(),
  markEngineVersionDismissed: vi.fn(),
}));

/** Walk a vnode tree, returning every node whose className contains `cls`. */
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

/** The payload an app iframe posts for `lucidos.ui.toast(...)`. */
const toastMsg = (payload: Record<string, unknown>) =>
  handleAppToastMessage('lucidos:ui:toast', payload);
/** The payload an app iframe posts for `lucidos.ui.dismissToast(key)`. */
const dismissMsg = (payload: Record<string, unknown>) =>
  handleAppToastMessage('lucidos:ui:dismissToast', payload);

beforeEach(() => {
  toasts.value = [];
  engineRestarting.value = false;
  vi.mocked(markSwUpdateDismissed).mockClear();
});

describe('app toast bridge: spinning', () => {
  // The whole point of the option: an app says "work in progress" and the host
  // renders the indeterminate spinner in place of the severity icon.
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

  // Everything on this payload crossed a postMessage boundary from app code, so
  // a non-boolean must not arrive as a truthy `spinning`.
  it('ignores a non-boolean spinning rather than treating it as true', () => {
    toastMsg({ message: 'Reindexing…', type: 'info', spinning: 'yes' });
    expect(toasts.value[0].spinning).toBeUndefined();
    expect(findByClass(ToastList(), 'mini-spinner').length).toBe(0);
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

  // An app can't know whether the toast is still up: the user may have closed it,
  // or its duration may have expired. "Already gone" is the normal case.
  it('is a silent no-op for a key matching nothing', () => {
    toastMsg({ message: 'Syncing…', type: 'info', key: 'sync' });
    const before = toasts.value;
    expect(() => dismissMsg({ key: 'no-such-toast' })).not.toThrow();
    // Same array identity: `removeToast` only reassigns when the key is present,
    // so an app polling dismiss can't wake every toast subscriber each call.
    expect(toasts.value).toBe(before);
  });

  it('is a no-op for a missing or non-string key', () => {
    toastMsg({ message: 'Syncing…', type: 'info', key: 'sync' });
    expect(() => dismissMsg({})).not.toThrow();
    expect(() => dismissMsg({ key: 42 })).not.toThrow();
    expect(toasts.value.length).toBe(1);
  });

  // The security-relevant half. `dismissToast`'s string arm records the running
  // build as user-dismissed for two host-owned keys, so routing an app's request
  // through it would let any app suppress the user's own Lucidos update prompt.
  it('does not fire the host user-dismiss side effect for a host-owned key', () => {
    showToast('New version available', 'info', { key: 'update-available' });
    dismissMsg({ key: 'update-available' });
    expect(toasts.value.length).toBe(0);
    expect(markSwUpdateDismissed).not.toHaveBeenCalled();
  });

  // The contrast that keeps the assertion above honest: the HOST path (the user
  // clicking the toast's close button) does still record the dismissal.
  it('but the host dismiss path still does', () => {
    showToast('New version available', 'info', { key: 'update-available' });
    dismissToast('update-available');
    expect(markSwUpdateDismissed).toHaveBeenCalledTimes(1);
  });
});

describe('app toast bridge: message ownership', () => {
  // `useStartup` routes on this boolean, and it is what lets the dismiss branch
  // sit ahead of the "confirm and prompt carry a message" guard.
  it('claims both toast messages and no others', () => {
    expect(toastMsg({ message: 'x' })).toBe(true);
    expect(dismissMsg({ key: 'x' })).toBe(true);
    expect(handleAppToastMessage('lucidos:ui:confirm', { message: 'x' })).toBe(false);
    expect(handleAppToastMessage('lucidos:ui:prompt', { message: 'x' })).toBe(false);
    expect(handleAppToastMessage('lucidos:ui:preview-file', {})).toBe(false);
  });

  // Claimed, but nothing to show: a toast without a message is malformed, not a
  // confirm, so it must not fall through to the branches below it.
  it('claims a message-less toast without showing anything', () => {
    expect(toastMsg({ type: 'info' })).toBe(true);
    expect(toasts.value.length).toBe(0);
  });
});
