import { describe, it, expect, beforeEach } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
import { Toast } from '../Toast';
import { toasts, showToast, engineRestarting } from '../../../store/store';
import { initiateEngineRestart, RESTART_TOAST_KEY } from '../../../store/actions/chat-changes';

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

describe('Toast close button — gated by dismissable flag', () => {
  it('omits .toast-close when dismissable: false', () => {
    showToast('Restarting engine...', 'info', { key: RESTART_TOAST_KEY, spinning: true, dismissable: false });
    expect(findByClass(Toast(), 'toast-close').length).toBe(0);
  });

  it('renders .toast-close on warning toasts (dismissable defaults to true)', () => {
    showToast('Engine restart required.', 'warning', {
      key: RESTART_TOAST_KEY,
      action: { label: 'Restart', onClick: () => {} },
    });
    expect(findByClass(Toast(), 'toast-close').length).toBe(1);
  });

  it('renders .toast-close on plain info toasts (no opts → dismissable defaults to true)', () => {
    showToast('Hello', 'info');
    expect(findByClass(Toast(), 'toast-close').length).toBe(1);
  });

  it('the close button click dismisses a dismissable toast', () => {
    showToast('Hello', 'info');
    const buttons = findByClass(Toast(), 'toast-close');
    expect(buttons.length).toBe(1);
    const onClick = (buttons[0].props as { onClick?: () => void }).onClick;
    onClick!();
    expect(toasts.value.length).toBe(0);
  });
});

describe('initiateEngineRestart raises a light dismissible status toast', () => {
  it('the build-phase status toast spins but stays dismissible (UI not deactivated during restart)', async () => {
    // Don't await — restartEngine() will try to hit the network and reject;
    // we only need the synchronous showToast that runs before the await.
    void initiateEngineRestart().catch(() => {});
    const toast = toasts.value.find(t => t.key === RESTART_TOAST_KEY);
    expect(toast).toBeDefined();
    // Dev (non-packaged) starts on the build phase with a progress spinner.
    expect(toast!.message).toBe('Building the new version…');
    expect(toast!.dismissable).not.toBe(false);
    expect(toast!.spinning).toBe(true);
    // And the rendered tree shows the close button so the user can dismiss it.
    expect(findByClass(Toast(), 'toast-close').length).toBe(1);
  });
});
