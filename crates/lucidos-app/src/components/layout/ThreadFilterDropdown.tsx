import { Fragment } from 'preact';
import type { ComponentType, VNode } from 'preact';
import { useRef, useEffect } from 'preact/hooks';
import {
  threadChannelFilter, selectedTriggerIds, selectedRepoIds, selectedAppIds, CODING_AGENT_CHANNEL,
  drawerView, setDrawerView, attentionThreadCount, reviewThreadCount, runningThreadCount,
  type DrawerView, type ThreadChannel,
  includeDeletedFilterOptions, setIncludeDeletedFilterOptions,
} from '../../store/store';
import { draftThreadCount } from '../drawer/family-graph';
import { DraftsIcon, AttentionIcon, ReviewIcon, RunningIcon, FilterIcon, CheckIcon, CodeIcon } from '../shared/icons';
import { LucidosMark } from '../shared/LucidosMark';
import { CategoryIcon } from '../shared/CategoryIcon';
import { CHANNEL_OPTIONS } from './headerHelpers';
import { toggleChannel, triggerFilterOptions, toggleTriggerId, toggleTriggerChannel, type TriggerFilterOption } from '../../store/triggerFilters';
import { repoFilterOptions, toggleRepoId, toggleCodingAgentChannel, type RepoFilterOption } from '../../store/repoFilters';
import { appFilterOptions, toggleAppId, type AppFilterOption } from '../../store/appFilters';
import { formatShortDateWithYear } from '../../utils/formatTime';
import { Overlay } from '../shared/Overlay';

/** Children rendered under an expanded parent share the same shape — all
 *  child options carry id/label/deleted/lastActivity. */
type ChildOption = TriggerFilterOption | RepoFilterOption | AppFilterOption;

type ChildGroup = {
  label: string;
  items: ChildOption[];
  selected: Set<string>;
  onToggleChild: (id: string) => void;
};

/** The five drawer views, in menu order — single-select. `all` is the default
 *  sectioned list; the other four are flat filtered views. Counts come from the
 *  store / family-graph (see `DrawerView` in store.ts). `all` shows no count. */
const VIEW_META: readonly { view: DrawerView; label: string; Icon: ComponentType<{ size?: string }> }[] = [
  { view: 'all', label: 'All statuses', Icon: FilterIcon },
  { view: 'attention', label: 'Needs attention', Icon: AttentionIcon },
  { view: 'review', label: 'Review', Icon: ReviewIcon },
  { view: 'running', label: 'Running', Icon: RunningIcon },
  { view: 'drafts', label: 'Drafts', Icon: DraftsIcon },
];

/** The icon for a given view — so the threads-header Filter button can reflect
 *  the selected view (the funnel `FilterIcon` for the default `all`, each view's
 *  own glyph otherwise). Single source of truth shared with the dropdown rows. */
export function viewIcon(view: DrawerView): ComponentType<{ size?: string }> {
  return (VIEW_META.find(m => m.view === view) ?? VIEW_META[0]).Icon;
}

/** The leading glyph for a Show-section channel row (Lucidos / Coding Agent /
 *  Triggers). Mirrors the per-thread `ThreadTypeIcon` mapping so a channel wears
 *  the same mark in the filter as the threads it gathers — except the Coding
 *  Agent group is backend-agnostic, so it uses the generic code glyph rather
 *  than a specific Claude/Codex mark. The Lucidos mark renders monochrome here
 *  (`background={false}` drops the gradient tile; CSS tints the squares + spark
 *  with the row color) so it reads as one of the line glyphs, not a brand badge. */
function channelIcon(value: ThreadChannel): VNode {
  if (value === 'trigger') return <CategoryIcon category="triggers" />;
  if (value === CODING_AGENT_CHANNEL) return <CodeIcon />;
  return <LucidosMark size="1rem" background={false} />;
}

/** The unified thread-drawer filter — one dropdown with two sections: a
 *  single-select **View** (All statuses / Needs attention / Review / Running / Drafts)
 *  on top, a divider, then the multi-select **Show** (channel) section. The
 *  channel section is disabled (greyed) whenever a non-`all` view is active —
 *  those views deliberately bypass the channel filter. Replaces the former pair
 *  of separate header controls (the view-selector menu + the channel dropdown).
 *
 *  Mounted only while open (so `open` is always true here). Anchor is the toggle
 *  button so re-clicking it routes through its own onClick toggle instead of
 *  being swallowed by dismiss. The full dismiss + swallow + Escape + inert
 *  contract lives in `<Overlay>`; `backdrop={false}` since this dropdown
 *  positions via CSS (anchored under the toggle's `.view-selector-slot`). */
export function ThreadFilterDropdown({ onClose, toggleRef }: { onClose: () => void; toggleRef: { current: HTMLButtonElement | null } }) {
  const view = drawerView.value;
  const filter = threadChannelFilter.value;
  const triggerChildren = triggerFilterOptions.value;
  const repoChildren = repoFilterOptions.value;
  const appChildren = appFilterOptions.value;
  const selectedTriggers = selectedTriggerIds.value;
  const selectedRepos = selectedRepoIds.value;
  const selectedApps = selectedAppIds.value;
  const includeDeleted = includeDeletedFilterOptions.value;

  const counts: Record<DrawerView, number> = {
    all: 0,
    attention: attentionThreadCount.value,
    review: reviewThreadCount.value,
    running: runningThreadCount.value,
    drafts: draftThreadCount.value,
  };
  // Channels apply only in the default `all` view — the alternate views bypass
  // them. Disable (grey out) the whole Show section rather than hiding it, so
  // the relationship stays legible (replaces the old "filter unavailable in
  // this view" disabled-button tooltip).
  const channelsDisabled = view !== 'all';

  return (
    <Overlay
      open
      onClose={onClose}
      anchor={toggleRef.current}
      backdrop={false}
      panelClass="thread-filter-dropdown"
    >
      <div class="thread-filter-title">View</div>
      {VIEW_META.map(({ view: v, label, Icon }) => {
        const count = counts[v];
        const isActive = v === view;
        return (
          <button
            key={v}
            class={`drawer-view-option${isActive ? ' drawer-view-option-active' : ''}`}
            role="menuitemradio"
            aria-checked={isActive}
            // Picking a view applies it and closes the dropdown — selecting a
            // status is a terminal choice, not a step the user keeps adjusting.
            onClick={() => { setDrawerView(v); onClose(); }}
          >
            <Icon />
            <span class="drawer-view-label">{label}</span>
            {/* Active marker hugs the label text — a single-select "you are here"
                checkmark sitting right after the name, so it reads as part of the
                text rather than riding the row's trailing edge or mirroring the
                Show section's leading toggle checkboxes. */}
            {isActive && (
              <span class="drawer-view-check"><CheckIcon /></span>
            )}
            {/* Only "Needs attention" wears the blue `badge`, so the standout
                count lines up with the badge on the threads-header filter icon;
                the other views show a plain muted number. The count is pinned to
                the row's trailing edge (CSS `margin-left:auto`), past the marker. */}
            {count > 0 && (
              <span class={`drawer-view-count${v === 'attention' ? ' badge' : ''}`}>{count}</span>
            )}
          </button>
        );
      })}

      <div class="thread-filter-divider" role="separator" />

      <div class={`thread-filter-title${channelsDisabled ? ' thread-filter-title-disabled' : ''}`}>Show</div>
      {channelsDisabled && (
        <div class="thread-filter-section-note">Unavailable in this view</div>
      )}
      {/* `<fieldset disabled>` natively disables every nested checkbox, so the
          channel rows can't be toggled while a non-`all` view is active; CSS
          greys it. Reset the fieldset's default chrome in `.thread-filter-show`. */}
      <fieldset class="thread-filter-show" disabled={channelsDisabled}>
        {CHANNEL_OPTIONS.map(opt => {
          const icon = channelIcon(opt.value);
          if (opt.value === 'trigger') {
            return (
              <ExpandableChannelRow
                key={opt.value}
                channelOn={filter.has('trigger')}
                label={opt.label}
                icon={icon}
                children={triggerChildren}
                selected={selectedTriggers}
                onToggleChild={toggleTriggerId}
                onToggleChannel={toggleTriggerChannel}
              />
            );
          }
          if (opt.value === CODING_AGENT_CHANNEL) {
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
                channelOn={filter.has(CODING_AGENT_CHANNEL)}
                label={opt.label}
                icon={icon}
                groups={groups}
                onToggleChannel={toggleCodingAgentChannel}
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
              <span class="thread-filter-channel-icon">{icon}</span>
              {opt.label}
            </label>
          );
        })}
        {/* Section-wide modifier: whether the expandable trigger/repo/app lists
            include entries whose underlying entity is gone (the `(deleted)` /
            `(until …)` rows). Off by default; persisted to localStorage. */}
        <div class="thread-filter-divider" role="separator" />
        <label class="thread-filter-option">
          <input
            type="checkbox"
            checked={includeDeleted}
            onChange={() => setIncludeDeletedFilterOptions(!includeDeleted)}
          />
          Include deleted
        </label>
      </fieldset>
    </Overlay>
  );
}

type ExpandableChannelRowProps = {
  channelOn: boolean;
  label: string;
  icon: VNode;
  onToggleChannel: () => void;
} & (
  | { children: ChildOption[]; selected: Set<string>; onToggleChild: (id: string) => void; groups?: undefined }
  | { groups: ChildGroup[]; children?: undefined; selected?: undefined; onToggleChild?: undefined }
);

function ExpandableChannelRow(props: ExpandableChannelRowProps) {
  const { channelOn, label, icon, onToggleChannel } = props;

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
        <span class="thread-filter-channel-icon">{icon}</span>
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
