import { Fragment } from 'preact';
import { useRef, useEffect } from 'preact/hooks';
import { threadChannelFilter, selectedTriggerIds, selectedRepoIds, selectedAppIds } from '../../store/store';
import { CHANNEL_OPTIONS } from './headerHelpers';
import { toggleChannel, triggerFilterOptions, toggleTriggerId, toggleTriggerChannel, type TriggerFilterOption } from '../../store/triggerFilters';
import { repoFilterOptions, toggleRepoId, toggleClaudeCodeChannel, type RepoFilterOption } from '../../store/repoFilters';
import { appFilterOptions, toggleAppId, type AppFilterOption } from '../../store/appFilters';
import { formatShortDateWithYear } from '../../utils/formatTime';
import { useDismissOnOutside } from '../../hooks/useAnchoredPopover';

/** Children rendered under an expanded parent share the same shape — all
 *  child options carry id/label/deleted/lastActivity. */
type ChildOption = TriggerFilterOption | RepoFilterOption | AppFilterOption;

type ChildGroup = {
  label: string;
  items: ChildOption[];
  selected: Set<string>;
  onToggleChild: (id: string) => void;
};

export function ThreadFilterDropdown({ onClose, toggleRef }: { onClose: () => void; toggleRef: { current: HTMLButtonElement | null } }) {
  const ref = useRef<HTMLDivElement>(null);
  const filter = threadChannelFilter.value;
  const triggerChildren = triggerFilterOptions.value;
  const repoChildren = repoFilterOptions.value;
  const appChildren = appFilterOptions.value;
  const selectedTriggers = selectedTriggerIds.value;
  const selectedRepos = selectedRepoIds.value;
  const selectedApps = selectedAppIds.value;

  // Component is mounted only while open, so isOpen=true is correct.
  // Anchor is the toggle button so re-clicking it routes through its own
  // onClick toggle instead of being swallowed by dismiss.
  useDismissOnOutside(true, ref, toggleRef.current, onClose);

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
          const groups: ChildGroup[] = [];
          if (repoChildren.length > 0) {
            groups.push({ label: 'Repos', items: repoChildren, selected: selectedRepos, onToggleChild: toggleRepoId });
          }
          if (appChildren.length > 0) {
            groups.push({ label: 'Apps', items: appChildren, selected: selectedApps, onToggleChild: toggleAppId });
          }
          return (
            <ExpandableChannelRow
              key={opt.value}
              channelOn={filter.has('claude_code')}
              label={opt.label}
              groups={groups}
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

type ExpandableChannelRowProps = {
  channelOn: boolean;
  label: string;
  onToggleChannel: () => void;
} & (
  | { children: ChildOption[]; selected: Set<string>; onToggleChild: (id: string) => void; groups?: undefined }
  | { groups: ChildGroup[]; children?: undefined; selected?: undefined; onToggleChild?: undefined }
);

function ExpandableChannelRow(props: ExpandableChannelRowProps) {
  const { channelOn, label, onToggleChannel } = props;

  // Normalize both shapes into a single `groups` view so the rest of the
  // function doesn't need to discriminate on the union per render. Single
  // groups render their items without the section header (length===1 below).
  const groups: ChildGroup[] = props.groups
    ? props.groups
    : [{
        label: '',
        items: props.children,
        selected: props.selected,
        onToggleChild: props.onToggleChild,
      }];

  // Flatten for the tri-state checkbox math. Lockstep below also keys off the
  // flattened total — a single child across all groups behaves as if there is
  // no per-child choice to make.
  const allItems: ChildOption[] = groups.flatMap(g => g.items);
  const selectionSize = groups.reduce((n, g) => n + g.selected.size, 0);

  // Lockstep: with a single child, "all" and "just this one" are identical
  // results, so parent and child mirror each other and clicking either
  // toggles the channel. The toggle handler is also lockstep-aware (it
  // bypasses the indeterminate-clear early-return) so stale selection from
  // a prior multi-child state doesn't make the click a no-op.
  const lockstep = allItems.length === 1;
  const effectiveSelectedSize = lockstep ? 0 : selectionSize;
  const checked = channelOn && effectiveSelectedSize === 0;
  const indeterminate = channelOn && effectiveSelectedSize > 0;
  const expanded = channelOn && allItems.length > 0;
  const showHeaders = groups.length > 1;

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
      {expanded && groups.map(group => (
        <Fragment key={group.label || 'default'}>
          {showHeaders && (
            <div class="dropdown-section-header">{group.label}</div>
          )}
          {group.items.map(child => (
            <ChildRow
              key={child.id}
              child={child}
              checked={lockstep ? channelOn : group.selected.has(child.id)}
              onChange={lockstep ? onToggleChannel : () => group.onToggleChild(child.id)}
            />
          ))}
        </Fragment>
      ))}
    </Fragment>
  );
}

function ChildRow({ child, checked, onChange }: { child: ChildOption; checked: boolean; onChange: () => void }) {
  const suffix = child.deleted
    ? (child.lastActivity
        ? `(until ${formatShortDateWithYear(new Date(child.lastActivity))})`
        : '(deleted)')
    : null;
  return (
    <label
      class={`thread-filter-option thread-filter-option-child${child.deleted ? ' thread-filter-option-deleted' : ''}`}
    >
      <input
        type="checkbox"
        checked={checked}
        onChange={onChange}
      />
      <span class="thread-filter-label">{child.label}</span>
      {suffix && <span class="thread-filter-deleted"> {suffix}</span>}
    </label>
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
