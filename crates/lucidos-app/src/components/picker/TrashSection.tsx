import { useSignal } from '@preact/signals';
import type { ComponentChildren } from 'preact';
import { useEffect, useRef } from 'preact/hooks';
import {
  abandonTrashStaging, confirmEmptyTrash, finishTrashDelete, freezeEmptyTrash, listTrash, restoreTrash, type TrashEntry,
} from '../../api/client/control';
import { showConfirm, showToast } from '../../store/store';
import { toFailed, type Loadable } from '../../store/types';
import { errorDetail } from '../../utils/errorDetail';
import { formatBytes } from '../../utils/formatBytes';
import { LoadableError } from '../shared/LoadableError';
import { LoadingFade } from '../shared/LoadingFade';
import { SkBlock, SkText, SkeletonProvider } from '../shared/Skeleton';
import { withProgressToast } from './progressToast';

function trashStatus(entry: TrashEntry): string {
  if (entry.finish_available) return 'Delete interrupted';
  if (entry.status === 'restoring') return 'Restore interrupted';
  if (entry.status === 'needs-attention') return 'Needs attention';
  return 'Deleted';
}

function TrashRow({ entry, children }: { entry?: TrashEntry; children?: ComponentChildren }) {
  const status = entry && trashStatus(entry);
  return (
    <li class="ws-picker-trash-row">
      <div class="ws-picker-trash-info">
        <span class="ws-picker-name"><SkText w="9rem">{entry?.name}</SkText></span>
        <span class={status && status !== 'Deleted' ? 'ws-picker-trash-status ws-picker-trash-status-warn' : 'ws-picker-trash-status'}>
          <SkText w="7rem">{entry && [status, entry.bytes != null && formatBytes(entry.bytes)].filter(Boolean).join(' · ')}</SkText>
        </span>
        {entry?.warning && <p class="ws-picker-trash-note">{entry.warning}</p>}
      </div>
      <div class="ws-picker-trash-actions">
        <SkBlock w="4rem" h="1.75rem" round>{children}</SkBlock>
      </div>
    </li>
  );
}

export function useTrashInventory(revision: number, enabled: boolean) {
  const trash = useSignal<Loadable<TrashEntry[]>>({ status: 'not-loaded' });
  const latestLoad = useRef(0);

  async function reload() {
    const load = ++latestLoad.current;
    try {
      const data = await listTrash();
      if (load === latestLoad.current) trash.value = { status: 'loaded', data };
    } catch (error) {
      if (load === latestLoad.current) trash.value = toFailed(error);
    }
  }

  useEffect(() => {
    if (enabled) void reload();
  }, [revision, enabled]);
  return { state: trash.value, reload };
}

export function TrashSection({ inventory, showSkeleton, onChanged }: {
  inventory: ReturnType<typeof useTrashInventory>;
  showSkeleton: boolean;
  onChanged: () => Promise<void>;
}) {
  const { state, reload } = inventory;
  const busy = useSignal(false);

  async function run(action: () => Promise<void>) {
    if (busy.value) return;
    busy.value = true;
    try {
      await action();
    } catch (error) {
      showToast(errorDetail(error), 'error');
    } finally {
      busy.value = false;
    }
  }

  async function mutate(action: () => Promise<void>) {
    try {
      await action();
    } finally {
      await reload();
      await onChanged();
    }
  }

  function restore(entry: TrashEntry) {
    void run(async () => {
      if (!await showConfirm(
        'Recovered worktrees use detached HEAD at the saved commit. The workspace stays stopped after recovery.',
        'Restore',
        { title: `Restore "${entry.name}"?`, variant: 'default' },
      )) return;
      await mutate(async () => {
        await withProgressToast('trash-action', `Restoring "${entry.name}"…`, restoreTrash(entry.id));
        showToast(`Restored "${entry.name}".`, 'success');
      });
    });
  }

  function finish(entry: TrashEntry) {
    void run(async () => {
      if (!await showConfirm(
        entry.database_backup
          ? 'Lucidos stops it, keeps a copy of its database in Trash, and removes it from the picker. Empty Trash to free the space.'
          : 'Lucidos stops it, deletes its database for good with no copy, and removes it from the picker.',
        'Finish delete',
        { title: `Finish deleting "${entry.name}"?`, variant: 'danger' },
      )) return;
      await mutate(async () => {
        await withProgressToast('trash-action', `Deleting "${entry.name}"…`, finishTrashDelete(entry.id));
        showToast(`Deleted "${entry.name}".`, 'success');
      });
    });
  }

  function resetStaging(entry: TrashEntry) {
    void run(async () => {
      if (!await showConfirm(
        'Lucidos keeps the old staging database. Retry the restore after the reset succeeds.',
        'Reset staging',
        { title: `Reset staging for "${entry.name}"?`, variant: 'default' },
      )) return;
      await mutate(async () => {
        await abandonTrashStaging(entry.id);
        showToast(`Reset staging for "${entry.name}".`, 'success');
      });
    });
  }

  function empty(entries: TrashEntry[]) {
    void run(async () => {
      const batch = await freezeEmptyTrash();
      if (!await showConfirm(
        `Permanently remove ${batch.members.length} Trash entries and their backups?`,
        'Empty Trash',
        { title: 'Empty Trash?', variant: 'danger' },
      )) return;
      await mutate(async () => {
        const { results } = await withProgressToast('trash-action', 'Emptying Trash…', confirmEmptyTrash(batch.id, batch.phrase));
        const names = new Map(entries.map((entry) => [entry.id, entry.name]));
        const skipped = results.filter((result) => result.status !== 'purged');
        if (!skipped.length) showToast('Trash emptied.', 'success');
        for (const result of skipped) {
          showToast(`Could not remove "${names.get(result.id) ?? result.id}": ${result.detail ?? 'The gateway skipped this entry.'}`, 'error');
        }
      });
    });
  }

  return (
    <LoadingFade
      showSkeleton={showSkeleton}
      skeleton={<SkeletonProvider><div class="ws-picker-trash"><ul><TrashRow /></ul></div></SkeletonProvider>}
    >
      {state.status === 'failed' && (
        <LoadableError noun="Trash" error={state.error} onRetry={() => { void reload(); }} />
      )}
      {state.status === 'loaded' && state.data.length > 0 && (
        <section aria-labelledby="ws-picker-trash-heading" class="ws-picker-trash">
          <div class="ws-picker-trash-head">
            <h2 id="ws-picker-trash-heading">Trash</h2>
            <button type="button" class="ws-picker-trash-empty" disabled={busy.value} onClick={() => empty(state.data)}>
              Empty Trash
            </button>
          </div>
          <ul>
            {state.data.map((entry) => (
              <TrashRow entry={entry} key={entry.id}>
                {entry.status === 'restoring' && (
                  <button type="button" class="ws-picker-btn" disabled={busy.value} onClick={() => resetStaging(entry)}>
                    Reset staging
                  </button>
                )}
                {entry.restore_available && (
                  <button type="button" class="ws-picker-btn" disabled={busy.value} onClick={() => restore(entry)}>
                    Restore
                  </button>
                )}
                {entry.finish_available && (
                  <button type="button" class="ws-picker-btn ws-picker-btn-danger-soft" disabled={busy.value} onClick={() => finish(entry)}>
                    Finish delete
                  </button>
                )}
              </TrashRow>
            ))}
          </ul>
        </section>
      )}
    </LoadingFade>
  );
}
