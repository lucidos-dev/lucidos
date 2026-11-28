import { OverflowMenu } from '../shared/OverflowMenu';
import { ArchiveIcon, EditIcon, TrashIcon } from '../shared/icons';
import { archiveThreadFolder, createThreadFolder, deleteThreadFolder, renameThreadFolder } from '../../store/actions/threadFolders';
import type { ThreadFolder } from '../../store/threadFolders';

export function NewFolderButton() {
    return (
        <button type="button" class="drawer-archive-all"
                onClick={(e) => { e.stopPropagation(); void createThreadFolder(); }}>
            New folder
        </button>
    );
}

export function CustomFolderMenu({ folder }: { folder: ThreadFolder }) {
    return (
        <OverflowMenu
            ariaLabel={`Actions for the folder ${folder.name}`}
            stopPropagation
            extraClass="drawer-folder-menu"
            items={({ run }) => (
                <div data-surface-step="folder-menu">
                    <button type="button" class="thread-overflow-item" role="menuitem"
                        onClick={run(() => { void renameThreadFolder(folder.id); })}>
                        <EditIcon />
                        Rename…
                    </button>
                    <button type="button" class="thread-overflow-item" role="menuitem"
                        onClick={run(() => { void archiveThreadFolder(folder.id); })}>
                        <ArchiveIcon />
                        Archive folder
                    </button>
                    <div class="thread-overflow-divider" role="separator" />
                    <button type="button" class="thread-overflow-item" role="menuitem"
                        onClick={run(() => { void deleteThreadFolder(folder.id); })}>
                        <TrashIcon />
                        Delete folder
                    </button>
                </div>
            )}
        />
    );
}
