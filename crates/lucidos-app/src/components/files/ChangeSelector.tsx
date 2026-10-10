import { useState, useRef } from 'preact/hooks';
import {
  repoChanges, repoSelectedChangeId, repoChangesLoadingMore, selectedChange,
} from '../../store/store';
import { selectRepoChange, loadMoreRepoChanges } from '../../store/actions/repositories';
import { formatTimeAgo } from '../../utils/formatTime';
import { formatFileCount } from '../../utils/formatFileCount';
import type { Change } from '../../api/client';
import { useDelayedFlag, useDelayedLoading } from '../../hooks/useDelayedLoading';
import { DropdownChevron, DropdownSkeleton } from '../shared/Dropdown';
import { LoadableError } from '../shared/LoadableError';
import { LoadingFade } from '../shared/LoadingFade';
import { Overlay } from '../shared/Overlay';
import { ListSkeletonOf, SkText } from '../shared/Skeleton';

function changeLabel(change: Change): string {
  return (change.description || 'Claude Code changes').split('\n')[0];
}

/** One change in the menu. With no `change`, inside a `SkeletonProvider`, it
 *  is the placeholder a page still loading appends. */
function ChangeOption({ change, active = false, showResolved = false, onPick }: {
  change?: Change;
  active?: boolean;
  showResolved?: boolean;
  onPick?: () => void;
}) {
  return (
    <div class={`dropdown-option${active ? ' active' : ''}`} onClick={onPick}>
      <SkText class="change-option-desc" w="10rem">{change && changeLabel(change)}</SkText>
      <SkText class="change-option-meta" w="4rem">
        {change && formatFileCount(change.file_count)}
        {change && showResolved && change.resolved_at && ` · ${formatTimeAgo(new Date(change.resolved_at))}`}
      </SkText>
    </div>
  );
}

export function ChangeSelector() {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  const listRef = useRef<HTMLDivElement>(null);

  const loadable = repoChanges.value;
  const loadingMore = repoChangesLoadingMore.value;
  const showLoading = useDelayedLoading(loadable);
  const showLoadingMore = useDelayedFlag(loadingMore);
  if (loadable.status === 'failed') return <LoadableError error={loadable.error} noun="changes" />;

  const data = loadable.status === 'loaded' ? loadable.data : null;
  // Nothing to choose from: the control unmounts with its slot.
  if (data && data.pending.length === 0 && data.applied.length === 0) return null;

  const selectedId = repoSelectedChangeId.value;
  const selected = selectedChange.value;

  function handleScroll() {
    const el = listRef.current;
    if (!el || !data?.has_more || loadingMore) return;
    if (el.scrollTop + el.clientHeight >= el.scrollHeight - 30) {
      void loadMoreRepoChanges();
    }
  }

  function handleSelect(change: Change | null) {
    void selectRepoChange(change);
    setOpen(false);
  }

  return (
    <LoadingFade class="dropdown-slot" showSkeleton={showLoading} skeleton={<DropdownSkeleton w="7rem" />}>
      {data && (
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
            <DropdownChevron open={open} />
          </button>
          <Overlay
            open={open}
            onClose={() => setOpen(false)}
            anchor={ref.current}
            backdrop={false}
            panelClass="surface-box dropdown-menu change-selector-menu"
            panelRef={listRef}
            panelProps={{ onScroll: handleScroll }}
          >
            <div
              class={`dropdown-option${!selectedId ? ' active' : ''}`}
              onClick={() => handleSelect(null)}
            >
              Current state (no diff)
            </div>
            {data.pending.length > 0 && (
              <>
                <div class="dropdown-section-header">Pending</div>
                {data.pending.map(c => (
                  <ChangeOption key={c.id} change={c} active={c.id === selectedId} onPick={() => handleSelect(c)} />
                ))}
              </>
            )}
            {data.applied.length > 0 && (
              <>
                <div class="dropdown-section-header">Recently Applied</div>
                {data.applied.map(c => (
                  <ChangeOption key={c.id} change={c} active={c.id === selectedId} showResolved onPick={() => handleSelect(c)} />
                ))}
              </>
            )}
            {/* The next page's rows, drawn at the bottom of a list that already
                scrolls, so nothing above them moves. */}
            <LoadingFade showSkeleton={showLoadingMore} skeleton={<ListSkeletonOf count={2} row={() => <ChangeOption />} />}>
              {null}
            </LoadingFade>
            {!loadingMore && data.has_more && (
              <div class="dropdown-panel-loading-more" style="opacity: 0.4">
                Scroll for more
              </div>
            )}
          </Overlay>
        </div>
      )}
    </LoadingFade>
  );
}
