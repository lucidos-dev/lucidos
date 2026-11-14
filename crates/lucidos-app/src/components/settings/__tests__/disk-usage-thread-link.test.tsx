// @vitest-environment jsdom
/** Settings › Disk usage names each worktree by its thread, and the name opens
 *  that thread. The buttons beside it keep their own clicks. */
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

const focusThreadOrBootstrap = vi.fn();
vi.mock('../../../store/actions/threads', () => ({
  focusThreadOrBootstrap: (id: string) => focusThreadOrBootstrap(id),
}));

import { DiskUsagePage } from '../DiskUsagePage';

const WORKTREE = {
  thread_id: 'thread-uuid-1',
  thread_title: 'Stop worktree target/ disk bloat',
  worktree_path: '/tmp/wt',
  size_bytes: 1024,
  artifact_bytes: 512,
  last_activity: null,
  is_dirty: false,
  is_saved: false,
  is_active: false,
  is_finished: false,
};

let host: HTMLDivElement;

beforeEach(async () => {
  focusThreadOrBootstrap.mockClear();
  vi.stubGlobal('fetch', async (url: string) => ({
    ok: true,
    status: 200,
    json: async () => (url.endsWith('/worktrees')
      ? { worktrees: [WORKTREE] }
      : { free_bytes: 1, total_bytes: 2, workspace_data_bytes: 0, soft_threshold_bytes: 0, hard_threshold_bytes: 0 }),
  }));
  host = document.createElement('div');
  document.body.appendChild(host);
  await act(async () => {
    render(<DiskUsagePage />, host);
    for (let i = 0; i < 10; i++) await Promise.resolve();
  });
});

afterEach(() => {
  render(null, host);
  host.remove();
  vi.unstubAllGlobals();
});

const titleLink = () =>
  host.querySelector<HTMLButtonElement>('.disk-usage-row .list-row-name button.accent-link');

it('draws the thread title as a link', () => {
  expect(titleLink()?.textContent).toBe(WORKTREE.thread_title);
});

it('opens the worktree\'s thread when the title is tapped', () => {
  act(() => { titleLink()!.click(); });
  expect(focusThreadOrBootstrap).toHaveBeenCalledWith('thread-uuid-1');
});

it('opens no thread from the cleanup buttons', () => {
  const clean = [...host.querySelectorAll<HTMLButtonElement>('.disk-usage-row .action-btn')]
    .find((b) => b.textContent === 'Clean artifacts')!;
  act(() => { clean.click(); });
  expect(focusThreadOrBootstrap).not.toHaveBeenCalled();
});
