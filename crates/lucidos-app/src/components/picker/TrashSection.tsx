import { useSignal } from '@preact/signals';
import type { ComponentChildren } from 'preact';
import { useEffect, useRef } from 'preact/hooks';
import {
  abandonTrashStaging, confirmEmptyTrash, freezeEmptyTrash, listTrash, restoreTrash, type TrashEntry,
} from '../../api/client/control';
import { showConfirm, showToast } from '../../store/store';
import { toFailed, type Loadable } from '../../store/types';
import { errorDetail } from '../../utils/errorDetail';
import { LoadableError } from '../shared/LoadableError';
import { LoadingFade } from '../shared/LoadingFade';
import { SkBlock, SkText, SkeletonProvider } from '../shared/Skeleton';

function TrashRow({ entry, children }: { entry?: TrashEntry; children?: ComponentChildren }) {
  return (
    <li class="list-row">
      <div class="list-row-info">
        <strong><SkText w="9rem">{entry?.name}</SkText></strong>
        {entry?.warning && <p>{entry.warning}</p>}
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
        await restoreTrash(entry.id);
        showToast(`Restored "${entry.name}".`, 'success');
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
        const { results } = await confirmEmptyTrash(batch.id, batch.phrase);
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
          <h2 id="ws-picker-trash-heading" class="list-section-title">Trash</h2>
          <ul>
            {state.data.map((entry) => (
              <TrashRow entry={entry} key={entry.id}>
                {entry.status === 'restoring' && (
                  <button type="button" class="action-btn" disabled={busy.value} onClick={() => resetStaging(entry)}>
                    Reset staging
                  </button>
                )}
                {entry.restore_available && (
                  <button type="button" class="action-btn" disabled={busy.value} onClick={() => restore(entry)}>
                    Restore
                  </button>
                )}
              </TrashRow>
            ))}
          </ul>
          <div class="ws-picker-trash-actions">
            <button type="button" class="action-btn action-btn-danger" disabled={busy.value} onClick={() => empty(state.data)}>
              Empty Trash
            </button>
          </div>
        </section>
      )}
    </LoadingFade>
  );
}
