import { describe, it, expect } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
import { directoryPickerBody } from '../DirectoryPicker';
import type { BrowseResult } from '../../../api/client';
import type { Loadable } from '../../../store/types';

/** Flatten a vnode tree into a string with the class attribute preserved
 *  so we can assert on per-state CSS classes. Mirrors the pattern used in
 *  permission-card.test.tsx / question-card.test.tsx. */
function vnodeToText(node: ComponentChildren): string {
  if (node === null || node === undefined || typeof node === 'boolean') return '';
  if (typeof node === 'string' || typeof node === 'number') return String(node);
  if (Array.isArray(node)) return node.map(vnodeToText).join('');
  const v = node as VNode<{ children?: ComponentChildren; class?: string; ['data-state']?: string }>;
  const tag = typeof v.type === 'string' ? v.type : '';
  const cls = v.props?.class ? ` class="${v.props.class}"` : '';
  const state = v.props?.['data-state'] ? ` data-state="${v.props['data-state']}"` : '';
  const inner = vnodeToText(v.props?.children);
  return tag ? `<${tag}${cls}${state}>${inner}</${tag}>` : inner;
}

const NOOP = () => {};

function callBody(data: Loadable<BrowseResult>, currentPath = '/some/path', showLoading = false) {
  return directoryPickerBody({
    data,
    showLoading,
    currentPath,
    selectedIndex: -1,
    onGoUp: NOOP,
    onSelectDir: NOOP,
    onHoverIndex: NOOP,
  });
}

describe('directoryPickerBody (Loadable discipline)', () => {
  it('shows the skeleton only once the delay has elapsed (showLoading=true)', () => {
    const text = vnodeToText(callBody({ status: 'loading' }, '/some/path', true));
    expect(text).toContain('loading-skeleton');
    expect(text).toMatch(/data-state="loading"/);
  });

  it('loading before the delay (showLoading=false) renders nothing — no skeleton flash', () => {
    const text = vnodeToText(callBody({ status: 'loading' }, '/some/path', false));
    expect(text).not.toContain('loading-skeleton');
    expect(text).toBe('');
  });

  it('failed state renders an error UI (distinct from empty + carries the error message)', () => {
    const text = vnodeToText(callBody({ status: 'failed', error: 'Permission denied' }));
    expect(text).toContain('dir-picker-error');
    expect(text).toContain('Permission denied');
    expect(text).toMatch(/data-state="failed"/);
  });

  it('loaded-empty renders the empty UI (and NOT the skeleton/error classes)', () => {
    const data: Loadable<BrowseResult> = {
      status: 'loaded',
      data: { path: '/x', directories: [], is_git_repo: false },
    };
    const text = vnodeToText(callBody(data, '/x'));
    expect(text).toContain('dir-picker-empty');
    expect(text).not.toContain('loading-skeleton');
    expect(text).not.toContain('dir-picker-error');
    expect(text).toContain('No subdirectories');
  });

  it('loaded with directories renders rows (and NOT the empty/skeleton/error classes)', () => {
    const data: Loadable<BrowseResult> = {
      status: 'loaded',
      data: { path: '/x', directories: ['alpha', 'beta'], is_git_repo: false },
    };
    const text = vnodeToText(callBody(data, '/x'));
    expect(text).toContain('alpha');
    expect(text).toContain('beta');
    expect(text).toContain('dir-picker-row');
    // The "no subdirectories" empty message must not appear when there are dirs.
    expect(text).not.toContain('No subdirectories');
    expect(text).not.toContain('loading-skeleton');
    expect(text).not.toContain('dir-picker-error');
  });
});
