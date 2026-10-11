import { Fragment } from 'preact';
import type { ComponentType, VNode } from 'preact';
import { useRef, useEffect } from 'preact/hooks';
import {
  threadChannelFilter, selectedTriggerIds, selectedRepoIds, selectedAppIds, CODING_AGENT_CHANNEL,
  type ThreadChannel,
  includeDeletedFilterOptions, setIncludeDeletedFilterOptions,
} from '../../store/store';
import { FilterIcon, FilteredIcon, CodeIcon } from '../shared/icons';
import { LucidosMark } from '../shared/LucidosMark';
import { Explainer } from '../shared/Explainer';
import { Disclosure } from '../shared/Disclosure';
import { CategoryIcon } from '../shared/CategoryIcon';
import { CHANNEL_OPTIONS } from './headerHelpers';
import { toggleChannel, triggerFilterOptions, toggleTriggerId, toggleTriggerChannel, type TriggerFilterOption } from '../../store/triggerFilters';
import { repoFilterOptions, toggleRepoId, toggleCodingAgentChannel, type RepoFilterOption } from '../../store/repoFilters';
import { appFilterOptions, widgetFilterOptions, toggleAppId, type AppFilterOption } from '../../store/appFilters';
import { formatShortDateWithYear } from '../../utils/formatTime';

/** Children rendered under an expanded parent share the same shape — all
 *  child options carry id/label/deleted/lastActivity. */
type ChildOption = TriggerFilterOption | RepoFilterOption | AppFilterOption;

type ChildGroup = {
  label: string;
  items: ChildOption[];
  selected: Set<string>;
  onToggleChild: (id: string) => void;
};

type IconType = ComponentType<{ size?: string }>;

/** A glyph the Filter button can wear: the funnel, outline (`all`) or filled
 *  while thread types narrow the list (`filtered`). */
export type FilterGlyph = 'all' | 'filtered';

/** Every glyph the Filter button can wear. Its crossfade
 *  (`ThreadFilterButton`) keeps both mounted. The record type makes a missing
 *  glyph a compile error. */
export const FILTER_BUTTON_GLYPHS: Readonly<Record<FilterGlyph, IconType>> = {
  all: FilterIcon,
  filtered: FilteredIcon,
};

/** Everything the threads-header Filter button looks like, on both layouts: its
 *  glyph and whether it is pressed.
 *
 *  PRESSED (the `view-selector-active` highlight) means the panel is open, and
 *  nothing else: a filtered list must not look like a panel left open. The
 *  GLYPH says whether thread types narrow the list, open or closed. It carries
 *  no count: the Blocked count rides on the grouping button.
 *
 *  Never an X when open. An X on a header control reads as "close this pane".
 *
 *  Called by `ThreadFilterButton`, which feeds it the signals; kept pure here
 *  so it is testable. */
export function filterButtonState(opts: {
  panelOpen: boolean;
  /** A channel / trigger / repo / app selection is set (`threadFilterActive`). */
  channelFilterActive: boolean;
}): { glyph: FilterGlyph; pressed: boolean } {
  return {
    glyph: opts.channelFilterActive ? 'filtered' : 'all',
    pressed: opts.panelOpen,
  };
}

/** The leading glyph for a Thread type row (Lucidos / Coding Agent /
 *  Triggers). Mirrors the per-thread `ThreadTypeIcon` mapping so a channel wears
 *  the same mark in the filter as the threads it gathers — except the Coding
 *  Agent group is backend-agnostic, so it uses the generic code glyph rather
 *  than a specific Claude/Codex mark. The Lucidos mark renders monochrome here
 *  (`background={false}` drops the gradient tile; CSS tints the squares + spark
 *  with the row color) so it reads as one of the line glyphs, not a brand badge. */
function channelIcon(value: ThreadChannel): VNode {
  if (value === 'trigger') return <CategoryIcon category="triggers" />;
  if (value === CODING_AGENT_CHANNEL) return <CodeIcon />;
  return <LucidosMark size="var(--icon-size-sm)" background={false} />;
}

/** The thread filter: which thread types the Folders grouping lists. The
 *  multi-select channel rows come first, with **Include deleted** after them.
 *  No heading: the pane title already says "Filters", and types are all the
 *  panel filters by. It shapes the Folders grouping only: the Ongoing grouping
 *  lists every type, and the panel never shows under it
 *  (store/threadFilterPanel.ts).
 *
 *  It renders as a VIEW INSIDE THE THREAD DRAWER PANE (see `ThreadDrawer`),
 *  covering the pane's list area while it is up. That is why it is NOT an
 *  `<Overlay>`: nothing floats over the thread or content panes. A click over
 *  there is the user's own and must not be dismissed-and-swallowed, and
 *  nothing behind goes inert. The one overlay behavior it keeps is Escape,
 *  registered on the central `overlayStack` alongside the open state itself
 *  (store/threadFilterPanel.ts), restores from localStorage included.
 *
 *  It carries neither a title row nor a footer. The pane header says "Filters"
 *  while this is up. The visible way out is the header's own Filter button,
 *  held down while the panel is open (see `filterButtonState`).
 *
 *  Always mounted, inside the drawer's `.thread-filter-cover`, which hides it
 *  while shut (see `ThreadFilterCover`). Hook-free at its own level so the unit
 *  test can invoke it directly (the nested `ExpandableChannelRow` /
 *  `TriCheckbox` use hooks; this component must not). */
export function ThreadFilterPanel() {
  const filter = threadChannelFilter.value;
  const triggerChildren = triggerFilterOptions.value;
  const repoChildren = repoFilterOptions.value;
  const appChildren = appFilterOptions.value;
  const widgetChildren = widgetFilterOptions.value;
  const selectedTriggers = selectedTriggerIds.value;
  const selectedRepos = selectedRepoIds.value;
  const selectedApps = selectedAppIds.value;
  const includeDeleted = includeDeletedFilterOptions.value;

  return (
    <div class="thread-filter-panel" role="group" aria-label="Thread filters">
      <div class="thread-filter-types" role="group" aria-label="Thread types">
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
            if (widgetChildren.length > 0) {
              groups.push({ label: 'Widgets', items: widgetChildren, selected: selectedApps, onToggleChild: toggleAppId });
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
      </div>

      {/* The modifier for the expandable trigger / repo / app child lists
          above: whether they include entries whose underlying entity is gone
          (the `(deleted)` / `(until …)` rows). Off by default; persisted to
          localStorage. */}
      <label class="thread-filter-option">
        <input
          type="checkbox"
          checked={includeDeleted}
          onChange={() => setIncludeDeletedFilterOptions(!includeDeleted)}
        />
        Include deleted
        <Explainer title="Include deleted">
          <p>
            Expanding <strong>Triggers</strong> or <strong>Coding Agent</strong> lists the
            individual triggers, repos and apps you can filter by. Normally that list only
            offers ones that still exist.
          </p>
          <p>
            Turn this on to list the deleted ones too, marked <em>(deleted)</em>, or{' '}
            <em>(until …)</em> with the date they were last active. Their threads are still
            here: this is how you filter down to the work a trigger did before you removed it.
          </p>
        </Explainer>
      </label>
    </div>
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
      <Disclosure open={expanded}>
        {groups.map(group => (
          <Fragment key={group.label || 'default'}>
            {/* Its own class, not the change selector's
                `.dropdown-section-header`. That one stands on a dropdown's left
                edge, which here falls left of every row this heading spans. */}
            {showHeaders && (
              <div class="thread-filter-group-title">{group.label}</div>
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
      </Disclosure>
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
