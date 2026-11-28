import { describe, it, expect, beforeEach, vi } from 'vitest';

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

import { PREF_THREAD_FOLDERS } from '@lucidos/preference-catalog';
import { makeThreadState } from './threads-test-helpers';
import { archiveThread, moveThreadToFolder, saveThread } from '../../api/threads';
import { preferences, showConfirm, showPrompt, threadMap, toasts } from '../store';
import { savePreferenceAccepted } from './preferences';
import { parseThreadFolders, serializeThreadFolders, threadFolders } from '../threadFolders';
import { _resetRowIntentsForTesting } from '../rowIntents';
import { archiveThreadFolder, deleteThreadFolder, NEW_FOLDER, pinThreadToFolder } from './threadFolders';
import type { ThreadState } from '../thread-events';

vi.mock('../../api/threads', () => ({
  saveThread: vi.fn(),
  moveThreadToFolder: vi.fn(),
  archiveThread: vi.fn(),
}));

vi.mock('./preferences', () => ({ savePreferenceAccepted: vi.fn() }));

vi.mock('../store', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../store')>()),
  showConfirm: vi.fn(),
  showPrompt: vi.fn(),
}));

const W48 = { id: 'w48', name: 'W48' };

function setFolders(folders: { id: string; name: string }[]): void {
  preferences.value = { status: 'loaded', data: { [PREF_THREAD_FOLDERS.key]: serializeThreadFolders(folders) } };
}

function setThreads(...threads: ThreadState[]): void {
  threadMap.value = new Map(threads.map(t => [t.meta.id, t]));
}

function inFolder(id: string, folder: string | null, extra: Partial<ThreadState['meta']> = {}): ThreadState {
  return makeThreadState(id, { meta: { saved: true, section: 'inbox', pinFolder: folder, ...extra } });
}

function savedFolders(): { id: string; name: string }[] {
  const calls = vi.mocked(savePreferenceAccepted).mock.calls;
  return parseThreadFolders(calls[calls.length - 1]?.[1]);
}

beforeEach(() => {
  vi.mocked(saveThread).mockReset();
  vi.mocked(moveThreadToFolder).mockReset();
  vi.mocked(archiveThread).mockReset();
  vi.mocked(savePreferenceAccepted).mockReset().mockResolvedValue(true);
  vi.mocked(showConfirm).mockReset().mockResolvedValue(true);
  vi.mocked(showPrompt).mockReset();
  toasts.value = [];
  _resetRowIntentsForTesting();
  setFolders([W48]);
});

describe('pinThreadToFolder', () => {
  it('pins an unpinned thread straight into the folder', async () => {
    setThreads(makeThreadState('t'));
    vi.mocked(saveThread).mockResolvedValue({ summary_version: 1 });

    await pinThreadToFolder('t', 'w48');

    expect(saveThread).toHaveBeenCalledWith('t', 'w48');
    expect(threadMap.value.get('t')!.meta).toMatchObject({ saved: true, pinFolder: 'w48' });
  });

  it('moves a pinned thread, and back to Pinned with null', async () => {
    setThreads(inFolder('t', 'w48'));
    vi.mocked(moveThreadToFolder).mockResolvedValue({ summary_version: 1 });

    await pinThreadToFolder('t', null);

    expect(moveThreadToFolder).toHaveBeenCalledWith('t', null);
    expect(threadMap.value.get('t')!.meta.pinFolder).toBeNull();
  });

  it('puts the folder back when the move is refused', async () => {
    setThreads(inFolder('t', null));
    vi.mocked(moveThreadToFolder).mockRejectedValue(new Error('unknown folder'));

    await pinThreadToFolder('t', 'w48');

    expect(threadMap.value.get('t')!.meta.pinFolder).toBeNull();
    expect(toasts.value).toHaveLength(1);
  });

  it('creates the folder first for New folder, then pins into it', async () => {
    setThreads(makeThreadState('t'));
    vi.mocked(showPrompt).mockResolvedValue('  Project X ');
    vi.mocked(saveThread).mockResolvedValue({ summary_version: 1 });

    await pinThreadToFolder('t', NEW_FOLDER);

    const created = savedFolders();
    expect(created.map(f => f.name)).toEqual(['W48', 'Project X']);
    expect(saveThread).toHaveBeenCalledWith('t', created[1].id);
  });

  it('pins nothing when the engine refuses the new folder', async () => {
    setThreads(makeThreadState('t'));
    vi.mocked(showPrompt).mockResolvedValue('A name the engine refuses');
    vi.mocked(savePreferenceAccepted).mockResolvedValue(false);

    await pinThreadToFolder('t', NEW_FOLDER);

    expect(saveThread).not.toHaveBeenCalled();
  });

  it('pins nothing when New folder is cancelled', async () => {
    setThreads(makeThreadState('t'));
    vi.mocked(showPrompt).mockResolvedValue(null);

    await pinThreadToFolder('t', NEW_FOLDER);

    expect(savePreferenceAccepted).not.toHaveBeenCalled();
    expect(saveThread).not.toHaveBeenCalled();
  });
});

describe('deleteThreadFolder', () => {
  it('drops the folder from the list and touches no thread', async () => {
    setThreads(inFolder('t', 'w48'));

    await deleteThreadFolder('w48');

    expect(savedFolders()).toEqual([]);
    expect(moveThreadToFolder).not.toHaveBeenCalled();
    expect(threadMap.value.get('t')!.meta.saved).toBe(true);
  });
});

describe('archiveThreadFolder', () => {
  it('archives every root and removes the emptied folder', async () => {
    setThreads(inFolder('a', 'w48'), inFolder('b', 'w48'), inFolder('other', null));
    vi.mocked(archiveThread).mockImplementation(async (id) => ({ archived: [id], summary_versions: { [id]: 1 } }));

    await archiveThreadFolder('w48');

    expect(vi.mocked(archiveThread).mock.calls.map(c => c[0]).sort()).toEqual(['a', 'b']);
    expect(savedFolders()).toEqual([]);
  });

  it('archives a child of Home, which the drawer files as its own root', async () => {
    setThreads(makeThreadState('home', { meta: { home: true } }), inFolder('c', 'w48', { parentThreadId: 'home' }));
    vi.mocked(archiveThread).mockImplementation(async (id) => ({ archived: [id], summary_versions: { [id]: 1 } }));

    await archiveThreadFolder('w48');

    expect(vi.mocked(archiveThread).mock.calls.map(c => c[0])).toEqual(['c']);
  });

  it('keeps a refused thread and the folder, and says how many stayed', async () => {
    setThreads(inFolder('done', 'w48'), inFolder('running', 'w48', { status: 'running' }));
    vi.mocked(archiveThread).mockImplementation(async (id) => id === 'running'
      ? { archived: [], skipped: [{ thread_id: 'running', reason: 'running', message: 'it is still running' }] }
      : { archived: [id], summary_versions: { [id]: 1 } });

    await archiveThreadFolder('w48');

    expect(savePreferenceAccepted).not.toHaveBeenCalled();
    expect(threadMap.value.get('running')!.meta).toMatchObject({ saved: true, pinFolder: 'w48' });
    expect(threadMap.value.get('done')!.meta).toMatchObject({ saved: false, pinFolder: null });
    expect(toasts.value.map(t => t.message).join(' ')).toContain('One thread stayed in "W48"');
  });

  it('archives nothing when the user cancels', async () => {
    setThreads(inFolder('a', 'w48'));
    vi.mocked(showConfirm).mockResolvedValue(false);

    await archiveThreadFolder('w48');

    expect(archiveThread).not.toHaveBeenCalled();
    expect(threadFolders.value).toEqual([W48]);
  });
});
