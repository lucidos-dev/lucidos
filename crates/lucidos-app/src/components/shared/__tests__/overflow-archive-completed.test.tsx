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
afterEach(() => { act(() => render(null, host)); threadMap.value = new Map(); vi.restoreAllMocks(); });
async function mount(count: number) {
  vi.spyOn(api, 'archiveAllPreflight').mockResolvedValue({
    safe: [{ thread_id: 'child', title: 'Child' }], safe_thread_count: count, kept: {}, kept_thread_count: 0,
  });
  await act(async () => render(<ThreadOverflowItems threadId="parent" title="Parent" run={fn => () => fn()} />, host));
}
it('shows the server count directly before Archive and invokes the scoped action', async () => {
  const action = vi.spyOn(actions, 'handleArchiveCompleted').mockResolvedValue();
  await mount(5);
  const button = [...host.querySelectorAll('button')].find(b => b.textContent?.includes('Archive 5 completed sub-threads'))!;
  expect(button).toBeDefined();
  // The normal Archive row can be absent for unloaded thread metadata.
  expect(button.className).toBe('thread-overflow-item');
  act(() => button.click());
  expect(action).toHaveBeenCalledWith('parent', expect.objectContaining({ safe_thread_count: 5 }));
});
it('hides a loaded zero count', async () => {
  await mount(0);
  expect(host.textContent).not.toContain('completed sub-threads');
});
it('renders a failed read distinctly from zero', async () => {
  vi.spyOn(api, 'archiveAllPreflight').mockRejectedValue(new Error('offline'));
  await act(async () => render(<ThreadOverflowItems threadId="parent" title="Parent" run={fn => () => fn()} />, host));
  await act(async () => {});
  expect(host.textContent).toContain('Failed to load completed sub-threads');
});

it('does not refetch when unrelated summaries replace the workspace map', async () => {
  await mount(2);
  await act(async () => { threadMap.value = new Map(threadMap.value); });
  expect(api.archiveAllPreflight).toHaveBeenCalledTimes(1);
});

it('refreshes for a descendant summary and keeps the count through a failed refresh', async () => {
  threadMap.value = new Map([
    ['parent', makeThreadState('parent')],
    ['child', makeThreadState('child', { meta: { parentThreadId: 'parent', summaryVersion: 1 } })],
  ]);
  await mount(2);
  vi.mocked(api.archiveAllPreflight).mockRejectedValueOnce(new Error('offline'));
  await act(async () => {
    threadMap.value = new Map(threadMap.value).set('child',
      makeThreadState('child', { meta: { parentThreadId: 'parent', summaryVersion: 2 } }));
  });
  await act(async () => {});
  expect(api.archiveAllPreflight).toHaveBeenCalledTimes(2);
  expect(host.textContent).toContain('Archive 2 completed sub-threads');
  expect(host.textContent).not.toContain('Failed to load');
});
