// @vitest-environment jsdom
import { describe, it, expect, afterEach } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import type { ComponentChildren, VNode } from 'preact';
import { ProgressDialog, progressDialogBody } from '../ProgressDialog';
import { progressDialog } from '../../../store/store';
import { restartDialogState, appUpdateDialogState } from '../../../store/progressDialogCopy';
import type { ProgressDialogState } from '../../../store/types';


function walk(node: ComponentChildren, out: VNode[] = []): VNode[] {
  if (node === null || node === undefined || typeof node === 'boolean') return out;
  if (typeof node === 'string' || typeof node === 'number') return out;
  if (Array.isArray(node)) {
    for (const child of node) walk(child, out);
    return out;
  }
  const v = node as VNode<{ children?: ComponentChildren; icon?: ComponentChildren }>;
  out.push(v);
  walk(v.props?.icon, out);
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

  it('leads the title with the spinner, not the body', () => {
    const indeterminate = progressDialogBody({ state: base });
    const body = byClass(indeterminate, 'surface-body')[0];
    expect(byClass(body, 'mini-spinner')).toHaveLength(0);
    const head = walk(indeterminate).find((v) => (v.props as { title?: unknown }).title === base.title);
    expect(byClass((head?.props as { icon?: ComponentChildren }).icon, 'mini-spinner')).toHaveLength(1);
  });

  it('clamps the fill rather than painting past its track', () => {
    const over = progressDialogBody({ state: { ...base, progress: 4 } });
    const fill = byClass(over, 'progress-bar-fill')[0];
    expect((fill.props as { style?: { width?: string } }).style?.width).toBe('100%');
  });

  it('offers Cancel only while cancelling is still possible', () => {
    expect(byClass(progressDialogBody({ state: base }), 'action-btn-secondary')).toHaveLength(0);
    const cancellable = progressDialogBody({
      state: { ...base, cancel: { label: 'Cancel', onClick: () => {} } },
    });
    expect(byClass(cancellable, 'action-btn-secondary')).toHaveLength(1);
  });

  it('never renders a dismiss X: the operation runs whatever the user presses', () => {
    expect(byClass(progressDialogBody({ state: base }), 'toast-close')).toHaveLength(0);
  });
});

describe('the rendered progress dialog', () => {
  const host = document.createElement('div');

  afterEach(() => {
    act(() => render(null, host));
    progressDialog.value = { ...base, visible: false };
    host.remove();
  });

  it('keeps the shared anatomy and focus on the frame when Cancel disappears', () => {
    document.body.append(host);
    act(() => {
      progressDialog.value = { ...base, cancel: { label: 'Cancel', onClick: () => {} } };
      render(<ProgressDialog />, host);
    });
    const panel = document.querySelector<HTMLElement>('.progress-dialog')!;
    expect(panel.querySelector(':scope > .surface-head')).not.toBeNull();
    expect(panel.querySelector(':scope > .surface-body')).not.toBeNull();
    expect(panel.querySelector(':scope > .surface-foot')).not.toBeNull();
    expect(document.activeElement).toBe(panel.querySelector('button'));

    act(() => { progressDialog.value = { ...base, message: 'Restarting background services…' }; });
    expect(panel.querySelector('.surface-foot')).toBeNull();
    expect(panel.lastElementChild?.classList.contains('surface-body')).toBe(true);
    expect(panel.tabIndex).toBe(-1);
    expect(document.activeElement).toBe(panel);
  });
});

describe('the flows that own the dialog', () => {
  it('gives a restart a spinner and no way out', () => {
    const body = progressDialogBody({ state: restartDialogState(true) });
    expect(byClass(body, 'mini-spinner')).toHaveLength(1);
    expect(byClass(body, 'progress-bar-fill')).toHaveLength(0);
    expect(byClass(body, 'action-btn-secondary')).toHaveLength(0);
  });

  it('gives a sized download a bar and a Cancel', () => {
    const state = appUpdateDialogState(
      { version: '9.9.9', phase: 'downloading', downloaded: 50, total: 200 },
      () => {},
    );
    const body = progressDialogBody({ state });
    expect(byClass(body, 'progress-bar-fill')).toHaveLength(1);
    expect(byClass(body, 'action-btn-secondary')).toHaveLength(1);
  });

  it('takes the Cancel away once the install has committed', () => {
    const state = appUpdateDialogState({ version: '9.9.9', phase: 'installing' }, () => {});
    const body = progressDialogBody({ state });
    expect(byClass(body, 'action-btn-secondary')).toHaveLength(0);
    expect(byClass(body, 'mini-spinner')).toHaveLength(1);
  });
});
