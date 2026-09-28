// @vitest-environment jsdom
import { afterEach, beforeEach, describe, it, expect } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { directoryPickerBody } from '../DirectoryPicker';
import type { BrowseResult } from '../../../api/client';
import type { Loadable } from '../../../store/types';

const NOOP = () => {};
let host: HTMLDivElement;

function show(data: Loadable<BrowseResult>, currentPath = '/some/path', showLoading = false) {
  act(() => {
    render(
      <div class="dir-picker-list">
        {directoryPickerBody({
          data,
          showLoading,
          currentPath,
          selectedIndex: -1,
          onGoUp: NOOP,
          onSelectDir: NOOP,
          onHoverIndex: NOOP,
        })}
      </div>,
      host,
    );
  });
  return host.firstElementChild!;
}

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
});

describe('directoryPickerBody (Loadable discipline)', () => {
  it('draws directory rows as the skeleton once the delay has elapsed (showLoading=true)', () => {
    const list = show({ status: 'loading' }, '/some/path', true);
    expect(list.querySelectorAll('.loading-fade-skeleton .dir-picker-row').length).toBeGreaterThan(0);
    expect(list.querySelector('.loading-fade-skeleton .dir-picker-name .sk-bar')).not.toBeNull();
    expect(list.textContent).not.toContain('Loading');
  });

  it('loading before the delay (showLoading=false) renders nothing: no skeleton flash', () => {
    const list = show({ status: 'loading' }, '/some/path', false);
    expect(list.querySelector('.sk-bar')).toBeNull();
    expect(list.textContent).toBe('');
  });

  it('failed state renders an error UI (distinct from empty + carries the error message)', () => {
    const list = show({ status: 'failed', error: 'Permission denied' });
    const error = list.querySelector('.dir-picker-error');
    expect(error?.textContent).toContain('Permission denied');
    expect(error?.getAttribute('data-state')).toBe('failed');
  });

  it('loaded-empty renders the empty UI (and NOT the skeleton/error classes)', () => {
    const list = show({ status: 'loaded', data: { path: '/', directories: [], is_git_repo: false } }, '/');
    expect(list.querySelector('.dir-picker-empty')?.textContent).toBe('No subdirectories');
    expect(list.querySelector('.sk-bar')).toBeNull();
    expect(list.querySelector('.dir-picker-error')).toBeNull();
  });

  it('loaded with directories renders rows (and NOT the empty/skeleton/error classes)', () => {
    const list = show({ status: 'loaded', data: { path: '/x', directories: ['alpha', 'beta'], is_git_repo: false } }, '/x');
    const names = [...list.querySelectorAll('.dir-picker-row .dir-picker-name')].map((n) => n.textContent);
    expect(names).toEqual(['..', 'alpha', 'beta']);
    expect(list.textContent).not.toContain('No subdirectories');
    expect(list.querySelector('.sk-bar')).toBeNull();
    expect(list.querySelector('.dir-picker-error')).toBeNull();
  });
});
