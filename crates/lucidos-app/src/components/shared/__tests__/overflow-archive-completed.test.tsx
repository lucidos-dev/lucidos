// @vitest-environment jsdom
import { afterEach, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { ThreadOverflowItems } from '../ThreadOverflowItems';
import * as api from '../../../api/threads';
import * as actions from '../../../store/actions/archive-completed';
import { threadMap } from '../../../store/store';
import { makeThreadState } from '../../../store/actions/threads-test-helpers';

const host = document.createElement('div');
afterEach(() => {
  act(() => render(null, host));
  threadMap.value = new Map();
  vi.restoreAllMocks();
});

function mount(count: number) {
  threadMap.value = new Map([
    ['parent', makeThreadState('parent', { meta: { archivableSubThreadCount: count } })],
    ['child', makeThreadState('child', { meta: { parentThreadId: 'parent' } })],
  ]);
  act(() => render(<ThreadOverflowItems threadId="parent" title="Parent" run={fn => () => fn()} />, host));
}

function archiveButton(count: number) {
  return [...host.querySelectorAll('button')].find(b => b.textContent?.includes(`Archive ${count} completed sub-threads`));
}

it('draws the summary count on the first frame and reads nothing on open', () => {
  const read = vi.spyOn(api, 'archiveAllPreflight');
  mount(13);
  expect(archiveButton(13)).toBeDefined();
  expect(host.querySelector('.disclosure.is-rolling')).toBeNull();
  expect(read).not.toHaveBeenCalled();
});

it('follows a new summary count while the menu is open', () => {
  mount(13);
  act(() => {
    threadMap.value = new Map(threadMap.value).set('parent',
      makeThreadState('parent', { meta: { archivableSubThreadCount: 4 } }));
  });
  expect(archiveButton(4)).toBeDefined();
});

it('hides a zero count', () => {
  mount(0);
  expect(host.textContent).not.toContain('completed sub-threads');
});

it('the press invokes the scoped action for the parent', () => {
  const action = vi.spyOn(actions, 'handleArchiveCompleted').mockResolvedValue();
  mount(5);
  const button = archiveButton(5)!;
  expect(button.className).toBe('thread-overflow-item');
  act(() => button.click());
  expect(action).toHaveBeenCalledWith('parent');
});
