import { useRef, useEffect, useState } from 'preact/hooks';
import { threadChannelFilter, excludedTriggerIds, triggers } from '../../store/store';
import { loadedOr } from '../../store/types';
import { CHANNEL_OPTIONS, toggleChannel, toggleTriggerId, showAllTriggers, hideAllTriggers } from './headerHelpers';
import { loadTriggers } from '../../store/actions/triggers';
import { useDismissOnOutside } from '../../hooks/useAnchoredPopover';

const VIEWPORT_MARGIN_PX = 8;

export function ThreadFilterDropdown({ onClose, toggleRef }: { onClose: () => void; toggleRef: { current: HTMLButtonElement | null } }) {
  const ref = useRef<HTMLDivElement>(null);
  const [position] = useState(() => {
    const rect = toggleRef.current!.getBoundingClientRect();
    const right = Math.max(VIEWPORT_MARGIN_PX, window.innerWidth - rect.right);
    return {
      top: rect.bottom,
      right,
      maxWidth: window.innerWidth - right - VIEWPORT_MARGIN_PX,
    };
  });
  const filter = threadChannelFilter.value;

  useDismissOnOutside(true, ref, toggleRef.current, onClose);
  useEffect(() => {
    window.addEventListener('resize', onClose);
    return () => window.removeEventListener('resize', onClose);
  }, [onClose]);

  return (
    <div
      class="thread-filter-dropdown"
      ref={ref}
      style={{ top: `${position.top}px`, right: `${position.right}px`, maxWidth: `${position.maxWidth}px` }}
    >
      <div class="thread-filter-title">Show</div>
      {CHANNEL_OPTIONS.map(opt => (
        <label class="thread-filter-option" key={opt.value}>
          <input
            type="checkbox"
            checked={filter.has(opt.value)}
            onChange={() => toggleChannel(opt.value)}
          />
          {opt.label}
        </label>
      ))}
      {filter.has('trigger') && <TriggerSubList />}
    </div>
  );
}

function TriggerSubList() {
  useEffect(() => {
    if (triggers.value.status === 'not-loaded') loadTriggers();
  }, []);

  const loadable = triggers.value;
  const triggerList = loadedOr(loadable, []);
  const isLoading = loadable.status === 'loading';
  const isFailed = loadable.status === 'failed';
  if (!isLoading && !isFailed && triggerList.length === 0) return null;

  const excluded = excludedTriggerIds.value;
  const allTriggerIds = triggerList.map(t => t.id);
  const visibleCount = allTriggerIds.filter(id => !excluded.has(id)).length;
  const allChecked = visibleCount === allTriggerIds.length && allTriggerIds.length > 0;
  const noneChecked = visibleCount === 0 && allTriggerIds.length > 0;

  return (
    <div class="thread-filter-subgroup">
      <div class="thread-filter-subhead">
        <span>Triggers</span>
        {triggerList.length > 0 && (
          <button
            type="button"
            class="thread-filter-toggle-all"
            onClick={() => allChecked ? hideAllTriggers(allTriggerIds) : showAllTriggers()}
          >
            {allChecked ? 'None' : 'All'}
          </button>
        )}
      </div>
      {isLoading && <div class="thread-filter-hint">Loading…</div>}
      {isFailed && (
        <div class="thread-filter-hint error-text">Failed to load triggers: {loadable.error}</div>
      )}
      {!isLoading && !isFailed && (
        <>
          {triggerList.map(t => (
            <label class="thread-filter-option thread-filter-suboption" key={t.id}>
              <input
                type="checkbox"
                checked={!excluded.has(t.id)}
                onChange={() => toggleTriggerId(t.id)}
              />
              <span class="thread-filter-trigger-name">{t.name}</span>
            </label>
          ))}
          {noneChecked && (
            <div class="thread-filter-hint">No triggers selected — list will be empty.</div>
          )}
        </>
      )}
    </div>
  );
}
