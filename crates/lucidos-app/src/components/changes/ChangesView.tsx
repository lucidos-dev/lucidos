import { Fragment } from 'preact';
import { useRef, useCallback, useEffect } from 'preact/hooks';
import { useSignal } from '@preact/signals';
import { changes, appliedChanges, changesHasMore, changesLoadingMore, busyChangeIds, applyAllInProgress, showConfirm } from '../../store/store';
import { applySingleChange, discardSingleChange, applyAllChanges, discardAllChanges, revertChange, loadMoreChanges } from '../../store/actions/chat-changes';
import { viewChangeDiff } from '../../store/actions/repositories';
import { focusThreadOrBootstrap } from '../../store/actions/threads';
import type { Change } from '../../api/client';
import { formatTimeAgo } from '../../utils/formatTime';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { LoadableError } from '../shared/LoadableError';
import { ListSkeleton } from '../shared/ListSkeleton';
import { LoadingFade } from '../shared/LoadingFade';

/** Render a change description, preserving line breaks. */
function ChangeDescription({ description }: { description: string }) {
  const lines = (description || 'Claude Code changes').split('\n');
  return (
    <span class="title change-description">
      {lines.map((line, i) => (
        <Fragment key={i}>{line}{i < lines.length - 1 && <br />}</Fragment>
      ))}
    </span>
  );
}

/** Open the thread that produced a change, landing on the turn where the change
 *  originated (its `ChangeProposed` is stamped with `data-change-id` on that
 *  exchange) rather than the bottom of the thread — the change isn't necessarily
 *  the last turn. Uses focusThreadOrBootstrap so a thread outside the loaded
 *  window (old archived row, cross-workspace link) still opens. Exported for the
 *  unit test. */
export function openChangeThread(change: Change): void {
  if (!change.thread_id) return;
  focusThreadOrBootstrap(change.thread_id, { targetChangeId: change.id });
}

export function ChangesView() {
  const sentinelRef = useRef<HTMLDivElement>(null);
  const busyIds = useSignal<Set<string>>(new Set());

  const guardedAction = useCallback((id: string, action: (id: string) => Promise<void>) => {
    if (busyIds.value.has(id)) return;
    busyIds.value = new Set([...busyIds.value, id]);
    action(id).finally(() => {
      const next = new Set(busyIds.value);
      next.delete(id);
      busyIds.value = next;
    });
  }, []);

  const pendingLoadable = changes.value;
  const appliedLoadable = appliedChanges.value;
  const hasMore = changesHasMore.value;
  const loadingMore = changesLoadingMore.value;

  // Infinite scroll: observe a sentinel at the bottom of the applied list. The
  // real scroll container is the ancestor `.content-pane-body` (overflow-y: auto
  // in panels/shell.css), NOT this view's `.panel-content` — an `onScroll`
  // listener on `.panel-content` never fired because that element doesn't scroll
  // (scroll events don't bubble). Rooting the observer at `.content-pane-body`
  // (mirrors NotificationsView) loads the next page as the sentinel comes into
  // view. `loadMoreChanges` self-guards against concurrent calls and the
  // no-more-pages case, so a stray intersection is harmless.
  useEffect(() => {
    const sentinel = sentinelRef.current;
    if (!sentinel || !hasMore) return;
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries[0]?.isIntersecting) void loadMoreChanges();
      },
      { root: sentinel.closest('.content-pane-body'), threshold: 0 },
    );
    observer.observe(sentinel);
    return () => observer.disconnect();
  }, [hasMore]);

  // Both signals load and update in lockstep (refreshChangesState and the
  // ChangesUpdated SSE both set them together), so failure on one ≈ failure
  // on both. Pending drives the spinner — its `loading` window is what the
  // user is waiting on for the next render.
  const showLoading = useDelayedLoading(pendingLoadable);

  if (pendingLoadable.status === 'failed' || appliedLoadable.status === 'failed') {
    const err = pendingLoadable.status === 'failed' ? pendingLoadable.error
              : appliedLoadable.status === 'failed' ? appliedLoadable.error
              : 'Unknown error';
    return (
      <div class="panel-content">
        <LoadableError noun="changes" error={err} />
      </div>
    );
  }
  const bothLoaded = pendingLoadable.status === 'loaded' && appliedLoadable.status === 'loaded';

  return (
    <div class="panel-content">
      <LoadingFade showSkeleton={showLoading} skeleton={<ListSkeleton />}>
        {bothLoaded ? (() => {
          const pending = pendingLoadable.data;
          const applied = appliedLoadable.data;
          return pending.length === 0 && applied.length === 0 ? (
            <div class="empty-state">No changes</div>
          ) : (
            <>
              {pending.length > 1 && (
            <div class="changes-bulk-actions">
              {/* Apply/Discard All skip changes whose thread is still working
                  (server-side too); disable the buttons when none are eligible. */}
              <button class="action-btn action-btn-danger" disabled={applyAllInProgress.value || !pending.some(c => !c.thread_active)} onClick={() => void discardAllChanges()}>Discard All</button>
              <button class="action-btn action-btn-confirm" disabled={applyAllInProgress.value || !pending.some(c => !c.thread_active)} onClick={() => void applyAllChanges()}>
                {applyAllInProgress.value ? 'Applying...' : 'Apply All'}
              </button>
            </div>
          )}
          {pending.map(change => {
            const busy = busyIds.value.has(change.id) || busyChangeIds.value.has(change.id);
            // A change whose thread is mid-turn can't be applied/discarded —
            // doing so races (or yanks the worktree from) the live coding-agent
            // session. The server refuses it too (guard_change_action).
            const threadActive = !!change.thread_active;
            const activeTip = 'The coding agent is still working on this thread — wait for it to finish';
            return (
              <div
                class={`list-row change-row${change.thread_id ? ' clickable' : ''}`}
                key={change.id}
                onClick={change.thread_id ? () => openChangeThread(change) : undefined}
              >
                <div class="list-row-info">
                  {change.thread_title && <span class="list-row-label">{change.thread_title}</span>}
                  <ChangeDescription description={change.description} />
                  <span class="list-row-details">
                    {change.file_count} file{change.file_count !== 1 ? 's' : ''}
                    {change.requires_restart && ' · Requires engine restart'}
                    {!change.hardened && ' · Not hardened'}
                  </span>
                </div>
                <div class="list-row-actions">
                  <button class="action-btn" onClick={(e) => { e.stopPropagation(); void viewChangeDiff(change); }}>Diff</button>
                  <button class="action-btn action-btn-danger" disabled={busy || threadActive} data-tooltip={threadActive ? activeTip : undefined} onClick={(e) => { e.stopPropagation(); guardedAction(change.id, discardSingleChange); }}>Discard</button>
                  <button class="action-btn action-btn-confirm" disabled={busy || threadActive} data-tooltip={threadActive ? activeTip : (change.requires_restart ? 'Engine restart required for these changes to be applied correctly. You will be prompted to restart' : undefined)} onClick={(e) => { e.stopPropagation(); guardedAction(change.id, applySingleChange); }}>
                    {busy ? 'Applying...' : 'Apply'}
                  </button>
                </div>
              </div>
            );
          })}
          {applied.length > 0 && (
            <>
              <div class="list-section-title">Recently Applied</div>
              {applied.map(change => (
                <div
                  class={`list-row change-row${change.thread_id ? ' clickable' : ''}`}
                  key={change.id}
                  style="opacity: 0.7"
                  onClick={change.thread_id ? () => openChangeThread(change) : undefined}
                >
                  <div class="list-row-info">
                    {change.thread_title && <span class="list-row-label">{change.thread_title}</span>}
                    <ChangeDescription description={change.description} />
                    <span class="list-row-details">
                      {change.file_count} file{change.file_count !== 1 ? 's' : ''}
                      {change.requires_restart && ' · Requires engine restart'}
                      {change.resolved_at && ` · ${formatTimeAgo(new Date(change.resolved_at))}`}
                    </span>
                  </div>
                  <div class="list-row-actions">
                    {change.pre_merge_sha && (
                      <button class="action-btn" onClick={(e) => { e.stopPropagation(); void viewChangeDiff(change); }}>Diff</button>
                    )}
                    {change.status === 'applied' ? (
                      <button class="action-btn action-btn-danger" disabled={busyIds.value.has(change.id)} onClick={async (e) => {
                        e.stopPropagation();
                        if (await showConfirm('Revert this change? Any later applied changes that touch the same files may conflict.', 'Revert')) {
                          guardedAction(change.id, revertChange);
                        }
                      }}>Revert</button>
                    ) : (
                      <span class="list-row-details" style="font-size: 0.8rem">Reverted</span>
                    )}
                  </div>
                </div>
              ))}
              {hasMore && (
                <div
                  ref={sentinelRef}
                  class="dropdown-panel-loading-more"
                  style={loadingMore ? undefined : 'opacity: 0.4'}
                >
                  {loadingMore ? 'Loading more...' : 'Scroll for more'}
                </div>
              )}
            </>
          )}
            </>
          );
        })() : null}
      </LoadingFade>
    </div>
  );
}
