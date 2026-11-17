/**
 * The rows of the thread ⋯ menu (`ThreadOverflowMenu`), in a chunk of their
 * own. The menu opens only once it is in memory (ADR 0436).
 */
import { useEffect, useState } from 'preact/hooks';
import { archiveAllPreflight, type ArchiveAllPreflight } from '../../api/threads';
import { type Loadable, loadingIfFresh, failedIfFresh } from '../../store/types';
import { archivingCompleted, handleArchiveCompleted } from '../../store/actions/archive-completed';
import { LoadableError } from './LoadableError';
import { Disclosure } from './Disclosure';
import type { ComponentChildren } from 'preact';
import type { OverflowMenuContext } from './OverflowMenu';
import { CopyIcon, DownloadIcon, ArchiveIcon, CheckIcon, EditIcon, CurrentIcon, LocateIcon, MoveToTopIcon, PinIcon, SearchIcon, SetAsideIcon, StandingApplyIcon, TrashIcon } from './icons';
import { canRenameThread, promptRenameThread } from '../../store/actions/threadRename';
import { copyThreadRef, copyThreadTitle } from '../../utils/threadRef';
import { exportThread } from '../../utils/exportThread';
import { resolveChangeMenuActions, threadIsPinnable } from '../../store/actions/threadActions';
import type { ThreadAction } from '../../generated/thread-lifecycle';
import { focusThread, handleArchiveThread, handleSaveThread, handleUnarchiveThread, handleUnsaveThread } from '../../store/actions/threads';
import { handleDeleteThread } from '../../store/actions/threads-delete';
import { canMoveToTopLevel, handleDetachThread } from '../../store/actions/threads-detach';
import { exitItems } from '../../store/actions/threadBlockers';
import { SUB_THREAD_STATE, moreSubThreads } from '../../store/actions/blockerCopy';
import { threadMap, standingApplyThreadIds } from '../../store/store';
import { threadDisplayTitle } from '../../utils/threadTitle';

/** Blocking sub-threads the menu lists by name before it counts the rest. */
export const LISTED_SUB_THREADS = 3;

const CHANGE_ACTION_ICON: Partial<Record<ThreadAction, (armed: boolean) => ComponentChildren>> = {
  apply: () => <CheckIcon />,
  apply_when_settled: (armed) => <StandingApplyIcon armed={armed} />,
  set_aside: () => <SetAsideIcon />,
  discard: () => <TrashIcon />,
};

/** The menu's rows for `threadId`. `ThreadOverflowMenu` documents them. */
export function ThreadOverflowItems({ threadId, title, run, onShowInFolders, onFindInThread }: {
  threadId: string;
  title: string;
  run: OverflowMenuContext['run'];
  onShowInFolders?: () => void;
  onFindInThread?: () => void;
}) {
  const liveThread = threadMap.value.get(threadId);
  const saved = liveThread?.meta.saved ?? false;
  const pinnable = !!liveThread && threadIsPinnable(liveThread.meta);
  const renamable = canRenameThread(liveThread);
  const changeActions = resolveChangeMenuActions(threadId);
  const standingApplyArmed = standingApplyThreadIds.value.has(threadId);
  const movable = canMoveToTopLevel(threadId);
  const exits = exitItems(threadId);
  return (
    <div data-surface-step="thread-menu">
      {(onShowInFolders || onFindInThread) && (
        <>
          {onShowInFolders && (
            <button type="button" class="thread-overflow-item" role="menuitem" onClick={run(onShowInFolders)}>
              <LocateIcon />
              Show in Folders
            </button>
          )}
          {onFindInThread && (
            <button type="button" class="thread-overflow-item find-in-thread" role="menuitem" onClick={run(onFindInThread)}>
              <SearchIcon />
              Find in thread
            </button>
          )}
          <div class="thread-overflow-divider" role="separator" />
        </>
      )}
      {pinnable && (
        <>
          <button type="button" class="thread-overflow-item" role="menuitem"
            onClick={run(() => { if (saved) void handleUnsaveThread(threadId); else void handleSaveThread(threadId); })}>
            <PinIcon filled={saved} />
            {saved ? 'Unpin thread' : 'Pin thread'}
          </button>
          <div class="thread-overflow-divider" role="separator" />
        </>
      )}
      {renamable && (
        <>
          <button type="button" class="thread-overflow-item" role="menuitem" onClick={run(() => { void promptRenameThread(threadId); })}>
            <EditIcon />
            Rename…
          </button>
          <div class="thread-overflow-divider" role="separator" />
        </>
      )}
      <button type="button" class="thread-overflow-item" role="menuitem" onClick={run(() => copyThreadRef(threadId, title))}>
        <CopyIcon />
        Copy thread reference
      </button>
      <button type="button" class="thread-overflow-item" role="menuitem" onClick={run(() => copyThreadTitle(title))}>
        <CopyIcon />
        Copy thread title
      </button>
      <button type="button" class="thread-overflow-item" role="menuitem" onClick={run(() => { void exportThread(threadId, title); })}>
        <DownloadIcon />
        Download thread
      </button>
      {changeActions.length > 0 && <div class="thread-overflow-divider" role="separator" />}
      {changeActions.map((action) => {
        const toggle = action.kind === 'apply_when_settled';
        return (
          <button key={action.kind} type="button"
            role={toggle ? 'menuitemcheckbox' : 'menuitem'}
            aria-checked={toggle ? standingApplyArmed : undefined}
            class={action.kind === 'discard' ? 'thread-overflow-item thread-overflow-item-danger' : 'thread-overflow-item'}
            data-tooltip={action.tooltip}
            onClick={run(() => { void action.invoke(); })}>
            {CHANGE_ACTION_ICON[action.kind]?.(standingApplyArmed)}
            {action.label}
            {toggle && standingApplyArmed && (
              <span class="thread-overflow-check thread-overflow-check-end" aria-hidden="true">✓</span>
            )}
          </button>
        );
      })}
      {(movable || exits.archive || exits.unarchive || exits.delete) && <div class="thread-overflow-divider" role="separator" />}
      {movable && (
        <button type="button" class="thread-overflow-item" role="menuitem"
          onClick={run(() => { void handleDetachThread(threadId); })}>
          <MoveToTopIcon />
          Move to top level
        </button>
      )}
      <ArchiveCompletedItem threadId={threadId} run={run} />
      {exits.archive === 'enabled' && (
        <button type="button" class="thread-overflow-item" role="menuitem" onClick={run(() => { void handleArchiveThread(threadId); })}>
          <ArchiveIcon />
          Archive
        </button>
      )}
      {exits.archive === 'blocked' && <BlockedExit icon={<ArchiveIcon />} label="Archive" reason={exits.reason} />}
      {exits.unarchive && (
        <button type="button" class="thread-overflow-item" role="menuitem" onClick={run(() => { void handleUnarchiveThread(threadId); })}>
          <CurrentIcon />
          Move to Current
        </button>
      )}
      {exits.delete === 'enabled' && (
        <button type="button" class="thread-overflow-item thread-overflow-item-danger" role="menuitem"
          onClick={run(() => { void handleDeleteThread(threadId); })}>
          <TrashIcon />
          Delete thread
        </button>
      )}
      {exits.delete === 'blocked' && <BlockedExit icon={<TrashIcon />} label="Delete thread" danger reason={exits.reason} />}
      {exits.reason && <div class="thread-overflow-note" aria-hidden="true">{exits.reason}</div>}
      {exits.subThreads.slice(0, LISTED_SUB_THREADS).map(({ thread: subThread, blocker }) => (
        <button key={subThread.meta.id} type="button" class="thread-overflow-item" role="menuitem"
          onClick={run(() => { focusThread(subThread.meta.id); })}>
          <LocateIcon />
          <span class="thread-overflow-sub-thread">
            <span class="thread-overflow-sub-thread-title">{threadDisplayTitle(subThread)}</span>
            <span class="thread-overflow-sub-thread-state">{SUB_THREAD_STATE[blocker]}</span>
          </span>
        </button>
      ))}
      {exits.subThreads.length > LISTED_SUB_THREADS && (
        <div class="thread-overflow-note">{moreSubThreads(exits.subThreads.length - LISTED_SUB_THREADS)}</div>
      )}
    </div>
  );
}

/** An exit the blocker holds back. `aria-disabled` rather than `disabled`, so it
 *  stays in the roving focus order. The note under the exits draws the reason
 *  once. Each exit also holds it as visually hidden text, so a screen reader
 *  reads it with the item. */
function BlockedExit({ icon, label, reason, danger }: {
  icon: ComponentChildren;
  label: string;
  reason: string | null;
  danger?: boolean;
}) {
  return (
    <button type="button" role="menuitem" aria-disabled="true"
      class={danger ? 'thread-overflow-item thread-overflow-item-blocked thread-overflow-item-danger' : 'thread-overflow-item thread-overflow-item-blocked'}>
      {icon}
      {label}
      {reason && <span class="visually-hidden">{` ${reason}`}</span>}
    </button>
  );
}

/** Mounted only while the menu is open. Subscribe to this subtree and its
 * ancestors; unrelated workspace updates do not change its count. */
function ArchiveCompletedItem({ threadId, run }: {
  threadId: string;
  run: OverflowMenuContext['run'];
}) {
  const summaries = threadMap.value;
  const children = new Map<string, string[]>();
  for (const [id, thread] of summaries) {
    const parent = thread.meta.parentThreadId;
    if (!parent) continue;
    const siblings = children.get(parent) ?? [];
    siblings.push(id);
    children.set(parent, siblings);
  }
  const related = new Set<string>();
  const pending = [threadId];
  while (pending.length) {
    const id = pending.pop()!;
    if (related.has(id)) continue;
    related.add(id);
    pending.push(...(children.get(id) ?? []));
  }
  let ancestor = summaries.get(threadId)?.meta.parentThreadId;
  while (ancestor && !related.has(ancestor)) {
    related.add(ancestor);
    ancestor = summaries.get(ancestor)?.meta.parentThreadId;
  }
  const version = [...related].sort().map(id => `${id}:${summaries.get(id)?.meta.summaryVersion}`).join(',');
  const busy = archivingCompleted.value.has(threadId);
  const [preflight, setPreflight] = useState<Loadable<ArchiveAllPreflight>>({ status: 'not-loaded' });
  const [retry, setRetry] = useState(0);
  useEffect(() => {
    let current = true;
    setPreflight(loadingIfFresh);
    archiveAllPreflight(threadId).then(data => {
      if (current) setPreflight({ status: 'loaded', data });
    }).catch(error => { if (current) setPreflight(prev => failedIfFresh(prev, error)); });
    return () => { current = false; };
  }, [threadId, version, retry]);
  switch (preflight.status) {
    case 'not-loaded':
    case 'loading': return null;
    case 'failed': return <LoadableError noun="completed sub-threads" error={preflight.error} onRetry={() => setRetry(n => n + 1)} />;
    case 'loaded': {
      const { data } = preflight;
      return <Disclosure open={data.safe_thread_count > 0} appear><button type="button" class="thread-overflow-item" role="menuitem" disabled={busy}
        onClick={run(() => { void handleArchiveCompleted(threadId, data); })}>
        <ArchiveIcon />
        Archive {data.safe_thread_count} completed sub-threads
      </button></Disclosure>;
    }
  }
}
