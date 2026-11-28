import { focusedThreadId, showConfirm, showPrompt, showToast, threadMap } from '../store';
import { threadFolders, serializeThreadFolders, type ThreadFolder } from '../threadFolders';
import { isFamilyRoot } from '../../components/drawer/family-graph';
import { PREF_THREAD_FOLDERS } from '@lucidos/preference-catalog';
import { savePreferenceAccepted } from './preferences';
import { generateUuid } from '../../utils/uuid';
import { archiveThread, moveThreadToFolder, type ArchiveSkippedMember } from '../../api/threads';
import { collectThreadFamily } from './threadFamily';
import {
  abandonArchive, focusOffCascade, formatArchiveSkippedToast, handleSaveThread, holdArchivedRows,
  settleArchive, showArchiveError, subscriptionsStoppedByArchive, versionsBeyondCurrent, visibleCandidatesAround, writePin,
} from './threads';
import { rowRequestInFlight } from '../rowIntents';

export const NEW_FOLDER = '+new' as const;
export type FolderChoice = string | null | typeof NEW_FOLDER;

function folderName(folderId: string): string {
  return threadFolders.value.find(f => f.id === folderId)?.name ?? 'folder';
}

async function saveFolders(folders: readonly ThreadFolder[]): Promise<boolean> {
  return savePreferenceAccepted(PREF_THREAD_FOLDERS.key, serializeThreadFolders(folders));
}

async function askFolderName(message: string, title: string, okLabel: string, defaultValue?: string): Promise<string | null> {
  const answer = await showPrompt(message, { title, okLabel, defaultValue, placeholder: 'Folder name' });
  const name = answer?.trim();
  return name ? name : null;
}

export async function createThreadFolder(): Promise<ThreadFolder | null> {
  const name = await askFolderName('Name the new folder. It lists above Pinned.', 'New folder', 'Create');
  if (name === null) return null;
  const folder = { id: generateUuid(), name };
  return await saveFolders([...threadFolders.value, folder]) ? folder : null;
}

export async function renameThreadFolder(folderId: string): Promise<void> {
  const current = threadFolders.value.find(f => f.id === folderId);
  if (!current) return;
  const name = await askFolderName('Give this folder a new name.', 'Rename folder', 'Rename', current.name);
  if (name === null || name === current.name) return;
  await saveFolders(threadFolders.value.map(f => (f.id === folderId ? { ...f, name } : f)));
}

export async function deleteThreadFolder(folderId: string): Promise<void> {
  const name = folderName(folderId);
  if (!await showConfirm(`Delete the folder "${name}"? Its threads stay pinned and move to Pinned.`, 'Delete', { variant: 'default' })) {
    return;
  }
  await saveFolders(threadFolders.value.filter(f => f.id !== folderId));
}

async function resolveChoice(choice: FolderChoice): Promise<string | null | undefined> {
  if (choice !== NEW_FOLDER) return choice;
  return (await createThreadFolder())?.id;
}

export async function pinThreadToFolder(threadId: string, choice: FolderChoice): Promise<void> {
  const folder = await resolveChoice(choice);
  if (folder === undefined) return;
  const thread = threadMap.value.get(threadId);
  if (!thread) return;
  if (!thread.meta.saved) {
    await handleSaveThread(threadId, folder ?? undefined);
    return;
  }
  if ((thread.meta.pinFolder ?? null) === folder) return;
  await writePin(threadId, 'move', { pinFolder: folder }, () => moveThreadToFolder(threadId, folder));
}

export function folderRootIds(folderId: string): string[] {
  const map = threadMap.value;
  return [...map.values()]
    .filter(t => t.meta.saved && t.meta.pinFolder === folderId && isFamilyRoot(t, map))
    .map(t => t.meta.id);
}

function keptMessage(kept: number, name: string): string {
  return kept === 1
    ? `One thread stayed in "${name}", so the folder stays.`
    : `${kept} threads stayed in "${name}", so the folder stays.`;
}

export async function archiveThreadFolder(folderId: string): Promise<void> {
  const name = folderName(folderId);
  const roots = folderRootIds(folderId);
  if (roots.length > 0) {
    const what = roots.length === 1 ? 'the thread' : `the ${roots.length} threads`;
    if (!await showConfirm(`Archive ${what} in "${name}"? Archiving unpins them, and the empty folder goes.`, 'Archive', { variant: 'default' })) {
      return;
    }
  }
  const cascades = new Map(roots.map(id => [id, collectThreadFamily(id)]));
  const everything = new Set([...cascades.values()].flatMap(c => [...c]));
  const stopping = subscriptionsStoppedByArchive(everything, '');
  if (stopping && !await showConfirm(stopping.message, 'Archive', { variant: 'default', cancelLabel: 'Cancel', details: stopping.details })) {
    return;
  }

  const focused = focusedThreadId.value;
  const candidates = focused && everything.has(focused) ? visibleCandidatesAround(focused) : null;
  let kept = 0;
  const archiving: [string, Set<string>][] = [];
  for (const [root, cascade] of cascades) {
    if (rowRequestInFlight(root)) kept++;
    else archiving.push([root, cascade]);
  }
  const legacyVersions = versionsBeyondCurrent(archiving.flatMap(([, c]) => [...c]));
  for (const [, cascade] of archiving) holdArchivedRows(cascade);
  if (candidates) focusOffCascade(candidates, everything);

  const skippedAll: ArchiveSkippedMember[] = [];
  for (const [root, cascade] of archiving) {
    try {
      const { skipped, skippedIds } = settleArchive(cascade, await archiveThread(root), legacyVersions);
      skippedAll.push(...skipped);
      if (skippedIds.has(root)) kept++;
    } catch (e) {
      abandonArchive(cascade);
      showArchiveError(e, root);
      kept++;
    }
  }
  if (skippedAll.length > 0) showToast(formatArchiveSkippedToast(skippedAll), 'warning');
  if (kept > 0) {
    showToast(keptMessage(kept, name), 'info');
    return;
  }
  await saveFolders(threadFolders.value.filter(f => f.id !== folderId));
}
