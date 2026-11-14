import { afterEach, expect, it, vi } from 'vitest';
import { archivingCompleted, handleArchiveCompleted } from './archive-completed';
import * as api from '../../api/threads';
import * as store from '../store';

const preflight: api.ArchiveAllPreflight = {
  safe: [{ thread_id: 'one', title: 'One' }, { thread_id: 'two', title: 'Two' }],
  safe_thread_count: 2, kept: {}, kept_thread_count: 0,
};
afterEach(() => { vi.restoreAllMocks(); archivingCompleted.value = new Set(); });

it('submits the counted ids with parent scope, without confirmation, and reports mixed results', async () => {
  const archive = vi.spyOn(api, 'archiveAll').mockResolvedValue({
    archived: ['one'], kept: [{ thread_id: 'two', reason: 'busy', slug: 'busy', thread_count: 1 }],
  });
  const confirm = vi.spyOn(store, 'showConfirm');
  const toast = vi.spyOn(store, 'showToast').mockImplementation(() => 'toast');
  await handleArchiveCompleted('parent', preflight);
  expect(archive).toHaveBeenCalledWith(['one', 'two'], 'parent');
  expect(confirm).not.toHaveBeenCalled();
  expect(toast).toHaveBeenCalledWith('Archived 1 completed sub-threads. Skipped 1.', 'success');
});

it('blocks duplicate submissions and surfaces failures, then permits retry', async () => {
  let reject!: (error: Error) => void;
  const archive = vi.spyOn(api, 'archiveAll').mockImplementation(() => new Promise((_, fail) => { reject = fail; }));
  const toast = vi.spyOn(store, 'showToast').mockImplementation(() => 'toast');
  const pending = handleArchiveCompleted('parent', preflight);
  await handleArchiveCompleted('parent', preflight);
  expect(archive).toHaveBeenCalledTimes(1);
  reject(new Error('offline'));
  await pending;
  expect(toast).toHaveBeenCalledWith(expect.stringContaining('0 archived'), 'error');
  expect(archivingCompleted.value.has('parent')).toBe(false);
});
