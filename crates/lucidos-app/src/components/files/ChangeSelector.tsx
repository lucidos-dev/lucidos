import { useState, useRef } from 'preact/hooks';
import {
  repoChanges, repoSelectedChangeId, repoChangesLoadingMore, selectedChange,
} from '../../store/store';
import { selectRepoChange, loadMoreRepoChanges } from '../../store/actions/repositories';
import { formatTimeAgo } from '../../utils/formatTime';
import { formatFileCount } from '../../utils/formatFileCount';
import type { Change } from '../../api/client';
import { LoadableError } from '../shared/LoadableError';
import { Overlay } from '../shared/Overlay';

function changeLabel(change: Change): string {
  return (change.description || 'Claude Code changes').split('\n')[0];
}

export function ChangeSelector() {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  const listRef = useRef<HTMLDivElement>(null);

  const loadable = repoChanges.value;
  if (loadable.status === 'failed') return <LoadableError error={loadable.error} noun="changes" />;
  if (loadable.status !== 'loaded') return null;

  const { pending, applied, has_more } = loadable.data;
  if (pending.length === 0 && applied.length === 0) return null;

  const selectedId = repoSelectedChangeId.value;
  const selected = selectedChange.value;
  const loadingMore = repoChangesLoadingMore.value;

  function handleScroll() {
    const el = listRef.current;
    if (!el || !has_more || loadingMore) return;
    if (el.scrollTop + el.clientHeight >= el.scrollHeight - 30) {
      void loadMoreRepoChanges();
    }
  }

  function handleSelect(change: Change | null) {
    void selectRepoChange(change);
    setOpen(false);
  }

  return (
    <div class="dropdown change-selector" ref={ref}>
      <button
        type="button"
        class="dropdown-trigger"
        onClick={() => setOpen(!open)}
      >
        <span class="dropdown-label-text">
          {selected
            ? changeLabel(selected)
            : 'View change...'}
        </span>
        <span class="dropdown-chevron">{open ? '\u25B4' : '\u25BE'}</span>
      </button>
      <Overlay
        open={open}
        onClose={() => setOpen(false)}
        anchor={ref.current}
        backdrop={false}
        panelClass="dropdown-menu change-selector-menu"
        panelRef={listRef}
        panelProps={{ onScroll: handleScroll }}
      >
          <div
            class={`dropdown-option${!selectedId ? ' active' : ''}`}
            onClick={() => handleSelect(null)}
          >
            Current state (no diff)
          </div>
          {pending.length > 0 && (
            <>
              <div class="dropdown-section-header">Pending</div>
              {pending.map(c => (
                <div
                  key={c.id}
                  class={`dropdown-option${c.id === selectedId ? ' active' : ''}`}
                  onClick={() => handleSelect(c)}
                >
                  <span class="change-option-desc">{changeLabel(c)}</span>
                  <span class="change-option-meta">
                    {formatFileCount(c.file_count)}
                  </span>
                </div>
              ))}
            </>
          )}
          {applied.length > 0 && (
            <>
              <div class="dropdown-section-header">Recently Applied</div>
              {applied.map(c => (
                <div
                  key={c.id}
                  class={`dropdown-option${c.id === selectedId ? ' active' : ''}`}
                  onClick={() => handleSelect(c)}
                >
                  <span class="change-option-desc">{changeLabel(c)}</span>
                  <span class="change-option-meta">
                    {formatFileCount(c.file_count)}
                    {c.resolved_at && ` \u00b7 ${formatTimeAgo(new Date(c.resolved_at))}`}
                  </span>
                </div>
              ))}
            </>
          )}
          {loadingMore && (
            <div class="dropdown-panel-loading-more">Loading more...</div>
          )}
          {!loadingMore && has_more && (
            <div class="dropdown-panel-loading-more" style="opacity: 0.4">
              Scroll for more
            </div>
          )}
      </Overlay>
    </div>
  );
}
