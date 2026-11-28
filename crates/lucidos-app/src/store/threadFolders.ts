import { computed } from '@preact/signals';
import { PREF_THREAD_FOLDERS } from '@lucidos/preference-catalog';
import { preferences } from './store';
import type { ThreadState } from './thread-events';

export interface ThreadFolder {
  id: string;
  name: string;
}

export type CustomFolderSectionKey = `folder:${string}`;

export function customFolderSectionKey(folderId: string): CustomFolderSectionKey {
  return `folder:${folderId}`;
}

export function parseThreadFolders(raw: string | null | undefined): ThreadFolder[] {
  if (!raw || !raw.trim()) return [];
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return [];
  }
  if (!Array.isArray(parsed)) return [];
  return parsed.filter((f): f is ThreadFolder =>
    !!f && typeof f === 'object' && typeof f.id === 'string' && typeof f.name === 'string');
}

export const threadFolders = computed<ThreadFolder[]>(() => {
  const prefs = preferences.value;
  return prefs.status === 'loaded' ? parseThreadFolders(prefs.data[PREF_THREAD_FOLDERS.key]) : [];
});

export function serializeThreadFolders(folders: readonly ThreadFolder[]): string {
  return JSON.stringify(folders.map(({ id, name }) => ({ id, name })));
}

export function knownFolderOf(thread: ThreadState | undefined, folders: readonly ThreadFolder[]): ThreadFolder | null {
  const id = thread?.meta.saved ? thread.meta.pinFolder : null;
  return id ? folders.find(f => f.id === id) ?? null : null;
}

export interface CustomFolderSplit {
  folders: { folder: ThreadFolder; threads: ThreadState[] }[];
  pinned: ThreadState[];
}

export function splitByCustomFolder(
  saved: readonly ThreadState[],
  rootByThread: ReadonlyMap<string, string>,
  byId: ReadonlyMap<string, ThreadState>,
  folders: readonly ThreadFolder[],
): CustomFolderSplit {
  const lists = new Map(folders.map(folder => [folder.id, [] as ThreadState[]]));
  const pinned: ThreadState[] = [];
  for (const thread of saved) {
    const root = byId.get(rootByThread.get(thread.meta.id) ?? thread.meta.id);
    const list = root?.meta.pinFolder ? lists.get(root.meta.pinFolder) : undefined;
    (list ?? pinned).push(thread);
  }
  return { folders: folders.map(folder => ({ folder, threads: lists.get(folder.id)! })), pinned };
}
