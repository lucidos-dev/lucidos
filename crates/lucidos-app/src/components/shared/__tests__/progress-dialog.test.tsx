import { describe, it, expect } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
import { progressDialogBody } from '../ProgressDialog';
import type { ProgressDialogState } from '../../../store/types';

/**
 * The progress dialog's pure body, which is hook-free for exactly this reason.
 *
 * Two properties matter, and neither needs a DOM. A determinate phase draws a
 * bar and an indeterminate one draws a spinner: a bar that invents a percentage
 * is a lie the user waits on. And the panel is always programmatically
 * focusable. That is what lets the container move focus off the button that
 * opened the dialog, even in a phase with no Cancel to focus.
 */

function walk(node: ComponentChildren, out: VNode[] = []): VNode[] {
  if (node === null || node === undefined || typeof node === 'boolean') return out;
  if (typeof node === 'string' || typeof node === 'number') return out;
  if (Array.isArray(node)) {
    for (const child of node) walk(child, out);
    return out;
  }
  const v = node as VNode<{ children?: ComponentChildren }>;
  out.push(v);
  walk(v.props?.children, out);
  return out;
}

function byClass(node: ComponentChildren, cls: string): VNode[] {
  return walk(node).filter((v) => {
    const c = (v.props as { class?: string } | undefined)?.class ?? '';
    return typeof c === 'string' && c.split(/\s+/).includes(cls);
  });
}

const base: ProgressDialogState = {
  visible: true,
  title: 'Updating Lucidos',
  message: 'Downloading…',
  progress: null,
};

describe('progressDialogBody', () => {
  it('draws a bar only when the phase has an honest percentage', () => {
    const determinate = progressDialogBody({ state: { ...base, progress: 0.5 } });
    expect(byClass(determinate, 'progress-bar-fill')).toHaveLength(1);
    expect(byClass(determinate, 'mini-spinner')).toHaveLength(0);
  });

  it('draws a spinner when it does not', () => {
    const indeterminate = progressDialogBody({ state: base });
    expect(byClass(indeterminate, 'progress-bar-fill')).toHaveLength(0);
    expect(byClass(indeterminate, 'mini-spinner')).toHaveLength(1);
  });

  it('clamps the fill rather than painting past its track', () => {
    const over = progressDialogBody({ state: { ...base, progress: 4 } });
    const fill = byClass(over, 'progress-bar-fill')[0];
    expect((fill.props as { style?: { width?: string } }).style?.width).toBe('100%');
  });

  it('offers Cancel only while cancelling is still possible', () => {
    expect(byClass(progressDialogBody({ state: base }), 'confirm-btn-cancel')).toHaveLength(0);
    const cancellable = progressDialogBody({
      state: { ...base, cancel: { label: 'Cancel', onClick: () => {} } },
    });
    expect(byClass(cancellable, 'confirm-btn-cancel')).toHaveLength(1);
  });

  it('never renders a dismiss X: the operation runs whatever the user presses', () => {
    expect(byClass(progressDialogBody({ state: base }), 'toast-close')).toHaveLength(0);
  });

  it('keeps the panel focusable without making it a Tab stop', () => {
    // The container focuses this when a phase has no Cancel, so focus leaves
    // the button that opened the dialog. -1 keeps it out of the tab order.
    const panel = byClass(progressDialogBody({ state: base }), 'progress-dialog-body')[0];
    expect((panel.props as { tabIndex?: number }).tabIndex).toBe(-1);
  });
});
