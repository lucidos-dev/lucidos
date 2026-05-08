import { Fragment } from 'preact';
import { useRef, useEffect } from 'preact/hooks';
import { threadChannelFilter, selectedTriggerIds, selectedRepoIds } from '../../store/store';
import { CHANNEL_OPTIONS } from './headerHelpers';
import { toggleChannel, triggerFilterOptions, toggleTriggerId, toggleTriggerChannel, type TriggerFilterOption } from '../../store/triggerFilters';
import { repoFilterOptions, toggleRepoId, toggleClaudeCodeChannel, type RepoFilterOption } from '../../store/repoFilters';
import { formatShortDateWithYear } from '../../utils/formatTime';

/** Children rendered under an expanded parent share the same shape — both
 *  trigger and repo options carry id/label/deleted/lastActivity. */
type ChildOption = TriggerFilterOption | RepoFilterOption;

export function ThreadFilterDropdown({ onClose, toggleRef }: { onClose: () => void; toggleRef: { current: HTMLButtonElement | null } }) {
  const ref = useRef<HTMLDivElement>(null);
  const filter = threadChannelFilter.value;
  const triggerChildren = triggerFilterOptions.value;
  const repoChildren = repoFilterOptions.value;
  const selectedTriggers = selectedTriggerIds.value;
  const selectedRepos = selectedRepoIds.value;

  useEffect(() => {
    // Capture phase + stopPropagation: the dismiss click must not also fire
    // the row/button beneath. preventDefault is intentionally NOT called so
    // links and text selection still work.
    function handleClick(e: MouseEvent) {
      if (toggleRef.current?.contains(e.target as Node)) return;
      if (ref.current?.contains(e.target as Node)) return;
      e.stopPropagation();
      onClose();
    }
    document.addEventListener('click', handleClick, true);
    return () => document.removeEventListener('click', handleClick, true);
  }, [onClose]);

  return (
    <div class="thread-filter-dropdown" ref={ref}>
      <div class="thread-filter-title">Show</div>
      {CHANNEL_OPTIONS.map(opt => {
        if (opt.value === 'trigger') {
          return (
            <ExpandableChannelRow
              key={opt.value}
              channelOn={filter.has('trigger')}
              label={opt.label}
              children={triggerChildren}
              selected={selectedTriggers}
              onToggleChild={toggleTriggerId}
              onToggleChannel={toggleTriggerChannel}
            />
          );
        }
        if (opt.value === 'claude_code') {
          return (
            <ExpandableChannelRow
              key={opt.value}
              channelOn={filter.has('claude_code')}
              label={opt.label}
              children={repoChildren}
              selected={selectedRepos}
              onToggleChild={toggleRepoId}
              onToggleChannel={toggleClaudeCodeChannel}
            />
          );
        }
        return (
          <label class="thread-filter-option" key={opt.value}>
            <input
              type="checkbox"
              checked={filter.has(opt.value)}
              onChange={() => toggleChannel(opt.value)}
            />
            {opt.label}
          </label>
        );
      })}
    </div>
  );
}

function ExpandableChannelRow({
  channelOn,
  label,
  children,
  selected,
  onToggleChild,
  onToggleChannel,
}: {
  channelOn: boolean;
  label: string;
  children: ChildOption[];
  selected: Set<string>;
  onToggleChild: (id: string) => void;
  onToggleChannel: () => void;
}) {
  // Lockstep: with a single child, "all" and "just this one" are identical
  // results, so parent and child mirror each other and clicking either
  // toggles the channel. The toggle handler is also lockstep-aware (it
  // bypasses the indeterminate-clear early-return) so stale selection from
  // a prior multi-child state doesn't make the click a no-op.
  const lockstep = children.length === 1;
  const effectiveSelectedSize = lockstep ? 0 : selected.size;
  const checked = channelOn && effectiveSelectedSize === 0;
  const indeterminate = channelOn && effectiveSelectedSize > 0;
  const expanded = channelOn && children.length > 0;
  return (
    <Fragment>
      <label class="thread-filter-option">
        <TriCheckbox
          checked={checked}
          indeterminate={indeterminate}
          onChange={onToggleChannel}
        />
        {label}
      </label>
      {expanded && children.map(child => {
        const suffix = child.deleted
          ? (child.lastActivity
              ? `(until ${formatShortDateWithYear(new Date(child.lastActivity))})`
              : '(deleted)')
          : null;
        const childChecked = lockstep ? channelOn : selected.has(child.id);
        const childOnChange = lockstep
          ? onToggleChannel
          : () => onToggleChild(child.id);
        return (
          <label
            class={`thread-filter-option thread-filter-option-child${child.deleted ? ' thread-filter-option-deleted' : ''}`}
            key={child.id}
          >
            <input
              type="checkbox"
              checked={childChecked}
              onChange={childOnChange}
            />
            <span class="thread-filter-label">{child.label}</span>
            {suffix && <span class="thread-filter-deleted"> {suffix}</span>}
          </label>
        );
      })}
    </Fragment>
  );
}

/** HTML checkboxes only support `indeterminate` via DOM property, so set it
 *  imperatively after render whenever the prop changes. */
function TriCheckbox({ checked, indeterminate, onChange }: { checked: boolean; indeterminate: boolean; onChange: () => void }) {
  const ref = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (ref.current) ref.current.indeterminate = indeterminate;
  }, [indeterminate]);
  return (
    <input
      ref={ref}
      type="checkbox"
      checked={checked}
      onChange={onChange}
    />
  );
}
