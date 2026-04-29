import { useRef, useEffect } from 'preact/hooks';
import { threadChannelFilter, excludedTriggerIds, triggers } from '../../store/store';
import { loadedOr } from '../../store/types';
import { CHANNEL_OPTIONS, toggleChannel, toggleTriggerId, showAllTriggers, hideAllTriggers } from './headerHelpers';
import { loadTriggers } from '../../store/actions/triggers';

export function ThreadFilterDropdown({ onClose, toggleRef }: { onClose: () => void; toggleRef: { current: HTMLButtonElement | null } }) {
  const ref = useRef<HTMLDivElement>(null);
  const filter = threadChannelFilter.value;

  useEffect(() => {
    function handleClick(e: MouseEvent) {
      if (toggleRef.current?.contains(e.target as Node)) return;
      if (ref.current && !ref.current.contains(e.target as Node)) onClose();
    }
    document.addEventListener('click', handleClick);
    return () => document.removeEventListener('click', handleClick);
  }, [onClose]);

  return (
    <div class="thread-filter-dropdown" ref={ref}>
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

  const triggerList = loadedOr(triggers.value, []);
  const isLoading = triggers.value.status === 'loading';
  if (!isLoading && triggerList.length === 0) return null;

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
      {isLoading ? (
        <div class="thread-filter-hint">Loading…</div>
      ) : (
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
