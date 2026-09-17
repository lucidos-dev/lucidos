/** The delete dialog offers Archive, and choosing it archives instead.
 *
 *  Two things are load-bearing here. The offer is read off the same tagged
 *  actions the thread's action list renders, so an archived thread is never
 *  told to archive. And the button only records the choice: `ConfirmDialog`
 *  closes the visible dialog after the handler returns, so an archive confirm
 *  opened from inside it would be answered "no" on the way out.
 */

import { describe, it, expect, beforeEach, vi } from 'vitest';

// Polyfill localStorage before store.ts is imported at module level.
// vi.hoisted runs before any imports are resolved.
vi.hoisted(() => {
  const storage = new Map<string, string>();
  (globalThis as any).localStorage = {
    getItem: (k: string) => storage.get(k) ?? null,
    setItem: (k: string, v: string) => storage.set(k, v),
    removeItem: (k: string) => storage.delete(k),
    clear: () => storage.clear(),
    get length() { return storage.size; },
    key: (_i: number) => null,
  };
  if (typeof globalThis.document === 'undefined') {
    (globalThis as any).document = {};
  }
  if (!(globalThis.document as any).querySelector) {
    (globalThis.document as any).querySelector = () => null;
  }
  if (!(globalThis.document as any).querySelectorAll) {
    (globalThis.document as any).querySelectorAll = () => [];
  }
  if (typeof globalThis.requestAnimationFrame === 'undefined') {
    (globalThis as any).requestAnimationFrame = (cb: any) => { cb(); return 0; };
  }
});

import { makeThreadState } from './threads-test-helpers';
import { archiveThread, deletePreflight, deleteThreadFamily } from '../../api/threads';
import { _resetComposeDraftsForTesting } from '../composeDrafts';
import { archivingThreadIds, confirmState, focusedThreadId, threadMap } from '../store';
import { handleDeleteThread } from './threads-delete';

vi.mock('../../api/threads', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../api/threads')>()),
  archiveThread: vi.fn().mockResolvedValue({ archived: [] }),
  deleteThreadFamily: vi.fn().mockResolvedValue({ deleted: [], event_count: 0, memory_count: 0 }),
  deletePreflight: vi.fn(),
}));

vi.mock('../../components/chat/promptFocus', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../components/chat/promptFocus')>()),
  focusPromptNow: vi.fn(),
  composeHandlers: vi.fn(),
}));

/** Nothing to warn about, nothing blocking: the plain dialog. */
function cleanPreflight() {
  return {
    thread_count: 1,
    sub_thread_titles: [],
    memory_count: 0,
    has_unapplied_branch_work: false,
    has_applied_changes: false,
    backups_present: false,
    blocked_by: [],
  };
}

beforeEach(() => {
  vi.mocked(deletePreflight).mockResolvedValue(cleanPreflight());
  vi.mocked(archiveThread).mockClear();
  vi.mocked(deleteThreadFamily).mockClear();
  threadMap.value = new Map();
  _resetComposeDraftsForTesting();
  focusedThreadId.value = null;
  archivingThreadIds.value = new Set();
  confirmState.value = { visible: false, message: '', okLabel: 'Delete' };
  (globalThis as any).innerWidth = 1024;
});

/** Let the preflight settle so the dialog is up. */
async function openedDialog() {
  await vi.waitFor(() => expect(confirmState.value.visible).toBe(true));
  return confirmState.value;
}

/** What `ConfirmDialog` does on a click: run the extra action, then close the
 *  dialog with a `false`, which is the answer the OK button did not give. */
function chooseExtraAction() {
  const state = confirmState.value;
  state.extraAction?.onClick();
  state.resolve?.(false);
  confirmState.value = { visible: false, message: '', okLabel: 'Delete' };
}

describe('handleDeleteThread offers Archive', () => {
  it('archives instead, and deletes nothing', async () => {
    threadMap.value = new Map([
      ['t1', makeThreadState('t1', { meta: { section: 'inbox' } })],
    ]);

    const running = handleDeleteThread('t1');
    const dialog = await openedDialog();
    expect(dialog.extraAction?.label).toBe('Archive instead');
    expect(dialog.message).toContain('Archive keeps it instead');

    chooseExtraAction();
    await running;

    expect(archiveThread).toHaveBeenCalledWith('t1');
    expect(deleteThreadFamily).not.toHaveBeenCalled();
  });

  it('offers no Archive to a thread that is already archived', async () => {
    threadMap.value = new Map([
      ['t1', makeThreadState('t1', { meta: { section: 'archived' } })],
    ]);

    const running = handleDeleteThread('t1');
    const dialog = await openedDialog();
    expect(dialog.extraAction).toBeUndefined();
    expect(dialog.message).not.toContain('Archive');

    dialog.resolve?.(false);
    await running;

    expect(deleteThreadFamily).not.toHaveBeenCalled();
  });

  it('deletes when the user takes the Delete button', async () => {
    threadMap.value = new Map([
      ['t1', makeThreadState('t1', { meta: { section: 'inbox' } })],
    ]);

    const running = handleDeleteThread('t1');
    const dialog = await openedDialog();
    dialog.resolve?.(true);
    await running;

    expect(deleteThreadFamily).toHaveBeenCalledWith('t1');
    expect(archiveThread).not.toHaveBeenCalled();
  });
});
