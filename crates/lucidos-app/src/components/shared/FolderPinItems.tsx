import type { ComponentChildren } from 'preact';
import { FolderIcon, PinIcon } from './icons';
import type { OverflowMenuContext } from './OverflowMenu';
import { threadMap } from '../../store/store';
import { knownFolderOf, threadFolders } from '../../store/threadFolders';
import { NEW_FOLDER, pinThreadToFolder, type FolderChoice } from '../../store/actions/threadFolders';

export function FolderPinItems({ threadId, run, includePinned }: {
  threadId: string;
  run: OverflowMenuContext['run'];
  includePinned: boolean;
}) {
  const thread = threadMap.value.get(threadId);
  const folders = threadFolders.value;
  const current = thread?.meta.saved ? (knownFolderOf(thread, folders)?.id ?? null) : undefined;
  const item = (choice: FolderChoice, label: string, icon: ComponentChildren) => (
    <button key={choice ?? 'pinned'} type="button" class="thread-overflow-item" role="menuitemradio"
      aria-checked={choice === current}
      onClick={run(() => { void pinThreadToFolder(threadId, choice); })}>
      {icon}
      {label}
      {choice === current && <span class="thread-overflow-check thread-overflow-check-end" aria-hidden="true">✓</span>}
    </button>
  );
  return (
    <>
      {includePinned && item(null, 'Pinned', <PinIcon />)}
      {folders.map(folder => item(folder.id, folder.name, <FolderIcon />))}
      {item(NEW_FOLDER, 'New folder…', <FolderIcon />)}
    </>
  );
}
