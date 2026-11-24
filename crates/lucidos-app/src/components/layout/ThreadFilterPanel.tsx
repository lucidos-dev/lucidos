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

type ChildOption = TriggerFilterOption | RepoFilterOption | AppFilterOption;

type ChildGroup = {
  label: string;
  items: ChildOption[];
  selected: Set<string>;
  onToggleChild: (id: string) => void;
};

type IconType = ComponentType<{ size?: string }>;

export type FilterGlyph = 'all' | 'filtered';

export const FILTER_BUTTON_GLYPHS: Readonly<Record<FilterGlyph, IconType>> = {
  all: FilterIcon,
  filtered: FilteredIcon,
};

export function filterButtonState(opts: {
  panelOpen: boolean;
  channelFilterActive: boolean;
}): { glyph: FilterGlyph; pressed: boolean } {
  return {
    glyph: opts.channelFilterActive ? 'filtered' : 'all',
    pressed: opts.panelOpen,
  };
}

function channelIcon(value: ThreadChannel): VNode {
  if (value === 'trigger') return <CategoryIcon category="triggers" />;
  if (value === CODING_AGENT_CHANNEL) return <CodeIcon />;
  return <LucidosMark size="var(--icon-size-sm)" background={false} />;
}

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

  const groups: ChildGroup[] = props.groups
    ? props.groups
    : [{
        label: '',
        items: props.children,
        selected: props.selected,
        onToggleChild: props.onToggleChild,
      }];

  const allItems: ChildOption[] = groups.flatMap(g => g.items);
  const selectionSize = groups.reduce((n, g) => n + g.selected.size, 0);

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
