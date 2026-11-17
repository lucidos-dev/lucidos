import { describe, it, expect, beforeEach, vi } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
import { ToastList } from '../Toast';
import { toasts, showToast, engineRestarting, activeProgressDialog } from '../../../store/store';
import { initiateEngineRestart } from '../../../store/actions/chat-changes';

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

beforeEach(() => {
  toasts.value = [];
  engineRestarting.value = false;
});

describe('Toast close button: timed toasts always have a close control', () => {
  it('omits .toast-close when dismissable: false', () => {
    showToast('Downloading embedding model', 'info', { key: 'model-download', spinning: true, dismissable: false });
    expect(findByClass(ToastList(), 'toast-close').length).toBe(0);
  });

  it('renders .toast-close on a keyed toast that waits to be answered', () => {
    showToast('Engine restart required.', 'warning', {
      key: 'restart-required',
      action: { label: 'Restart', onClick: () => {} },
    });
    expect(findByClass(ToastList(), 'toast-close').length).toBe(1);
  });

  it('renders .toast-close on an error, which never times out', () => {
    showToast('Could not stop waiting', 'error');
    expect(findByClass(ToastList(), 'toast-close').length).toBe(1);
  });

  it('renders .toast-close on a plain success toast, which leaves on its own', () => {
    showToast('Trigger saved', 'success');
    expect(findByClass(ToastList(), 'toast-close').length).toBe(1);
  });

  it('renders .toast-close on a keyed toast its caller put on a timer', () => {
    showToast('Build finished', 'success', { key: 'build-done', autoDismissMs: 8000 });
    expect(findByClass(ToastList(), 'toast-close').length).toBe(1);
  });

  for (const type of ['success', 'info', 'warning', 'error'] as const) {
    for (const key of [undefined, 'timed-toast']) {
      for (const dismissable of [undefined, false]) {
        it(`closes timed ${type} immediately and cancels its timer (key=${key}, dismissable=${dismissable})`, () => {
          vi.useFakeTimers();
          try {
            const timerCount = vi.getTimerCount();
            showToast('Operation finished', type, { key, autoDismissMs: 8000, dismissable });
            expect(vi.getTimerCount()).toBe(timerCount + 1);
            const buttons = findByClass(ToastList(), 'toast-close');
            expect(buttons).toHaveLength(1);
            const props = buttons[0].props as unknown as { 'aria-label': string; onClick: () => void };
            expect(props['aria-label']).toBe('Dismiss');
            props.onClick();
            expect(toasts.value).toEqual([]);
            expect(vi.getTimerCount()).toBe(timerCount);
          } finally {
            vi.useRealTimers();
          }
        });
      }
    }
  }

  it('renders .toast-close on a timed toast with a button, so Escape has a way out', () => {
    showToast('Build finished', 'success', {
      key: 'build-done',
      autoDismissMs: 8000,
      action: { label: 'Open', onClick: () => {} },
    });
    expect(findByClass(ToastList(), 'toast-close').length).toBe(1);
  });

  it('renders .toast-close on a timed toast whose card opens a target', () => {
    showToast('Thread finished', 'success', { autoDismissMs: 8000, onClick: () => {} });
    expect(findByClass(ToastList(), 'toast-close').length).toBe(1);
  });

  it('renders .toast-close on a timed toast whose card runs its action', () => {
    showToast('Build finished', 'info', {
      autoDismissMs: 8000,
      action: { label: 'Open', onClick: () => {} },
    });
    expect(findByClass(ToastList(), 'toast-close').length).toBe(1);
  });

  it('the close button click dismisses a dismissable toast', () => {
    showToast('Could not stop waiting', 'error');
    const buttons = findByClass(ToastList(), 'toast-close');
    expect(buttons.length).toBe(1);
    const onClick = (buttons[0].props as { onClick?: () => void }).onClick;
    onClick!();
    expect(toasts.value.length).toBe(0);
  });
});

describe('initiateEngineRestart raises a dialog, not a toast', () => {
  it('narrates the restart on the modal dialog and adds nothing to the stack', async () => {
    // Don't await: restartEngine() will try to hit the network and reject. We
    // only need the synchronous state set before the await.
    void initiateEngineRestart().catch(() => {});
    // No new-version signals are set here, so this is a plain respawn.
    expect(activeProgressDialog.value.title).toBe('Restarting engine');
    expect(toasts.value).toHaveLength(0);
    expect(findByClass(ToastList(), 'toast-close').length).toBe(0);
  });
});
