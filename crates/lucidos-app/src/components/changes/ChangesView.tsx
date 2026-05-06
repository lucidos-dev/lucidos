import { useRef, useCallback } from 'preact/hooks';
import { useSignal } from '@preact/signals';
import { changes, appliedChanges, changesHasMore, changesLoadingMore, busyChangeIds, showConfirm } from '../../store/store';
import { applySingleChange, discardSingleChange, applyAllChanges, discardAllChanges, revertChange, loadMoreChanges } from '../../store/actions/chat-changes';
import { viewChangeDiff } from '../../store/actions/repositories';
import { formatTimeAgo } from '../../utils/formatTime';

/** Render a change description, preserving line breaks. */
function ChangeDescription({ description }: { description: string }) {
  const lines = (description || 'Claude Code changes').split('\n');
  return (
    <span class="title change-description">
      {lines.map((line, i) => (
        <>{line}{i < lines.length - 1 && <br />}</>
      ))}
    </span>
  );
}

export function ChangesView() {
  const listRef = useRef<HTMLDivElement>(null);
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

  const handleScroll = useCallback(() => {
    const el = listRef.current;
    if (!el) return;
    if (el.scrollTop + el.clientHeight >= el.scrollHeight - 50) {
      loadMoreChanges();
    }
  }, []);

  const pending = changes.value;
  const applied = appliedChanges.value;
  const hasMore = changesHasMore.value;
  const loadingMore = changesLoadingMore.value;

  return (
    <div class="panel-content" ref={listRef} onScroll={handleScroll}>
      {pending.length === 0 && applied.length === 0 ? (
        <div class="empty-state">No changes</div>
      ) : (
        <>
          {pending.length > 1 && (
            <div class="changes-bulk-actions">
              <button class="action-btn action-btn-danger" onClick={() => discardAllChanges()}>Discard All</button>
              <button class="action-btn action-btn-confirm" onClick={() => applyAllChanges()}>Apply All</button>
            </div>
          )}
          {pending.map(change => {
            const busy = busyIds.value.has(change.id) || busyChangeIds.value.has(change.id);
            return (
              <div class="list-row" key={change.id}>
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
                  <button class="action-btn" onClick={(e) => { e.stopPropagation(); viewChangeDiff(change); }}>Diff</button>
                  <button class="action-btn action-btn-danger" disabled={busy} onClick={() => guardedAction(change.id, discardSingleChange)}>Discard</button>
                  <button class="action-btn action-btn-confirm" disabled={busy} data-tooltip={change.requires_restart ? 'Engine restart required for these changes to be applied correctly. You will be prompted to restart' : undefined} onClick={() => guardedAction(change.id, applySingleChange)}>
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
                <div class="list-row" key={change.id} style="opacity: 0.7">
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
                      <button class="action-btn" onClick={(e) => { e.stopPropagation(); viewChangeDiff(change); }}>Diff</button>
                    )}
                    {change.status === 'applied' ? (
                      <button class="action-btn action-btn-danger" disabled={busyIds.value.has(change.id)} onClick={async () => {
                        if (await showConfirm('Revert this change? Any later applied changes that touch the same files may conflict.', 'Revert')) {
                          guardedAction(change.id, revertChange);
                        }
                      }}>Revert</button>
                    ) : (
                      <span class="secondary" style="font-size: 0.8em">Reverted</span>
                    )}
                  </div>
                </div>
              ))}
              {loadingMore && (
                <div class="dropdown-panel-loading-more">Loading more...</div>
              )}
              {!loadingMore && hasMore && (
                <div class="dropdown-panel-loading-more" style="opacity: 0.4">
                  Scroll for more
                </div>
              )}
            </>
          )}
        </>
      )}
    </div>
  );
}
