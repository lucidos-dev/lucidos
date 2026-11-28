import { useLayoutEffect, useRef, useState } from 'preact/hooks';
import { CrossfadeStack } from '../shared/CrossfadeStack';
import { GlyphBadge } from '../shared/GlyphBadge';
import { ToggleIconButton } from '../shared/ToggleIconButton';
import { labelWithBlockedCount } from '../shared/ThreadToggleButton';
import { FolderIcon, InboxIcon } from '../shared/icons';
import { FILTER_BUTTON_GLYPHS, filterButtonState, type FilterGlyph } from './ThreadFilterPanel';
import { threadFilterPanelOpen, toggleThreadFilterPanel } from '../../store/threadFilterPanel';
import { threadFilterActive } from '../../store/threadFilterActive';
import { blockedThreadCount, drawerGrouping, setDrawerGrouping, selectedOngoingGroup, setSelectedOngoingGroup, type DrawerGrouping } from '../../store/store';
import { ongoingGroupLists, ongoingGroupOnSwitch } from '../drawer/family-graph';

const FILTER_GLYPH_LAYERS = Object.entries(FILTER_BUTTON_GLYPHS).map(([key, Icon]) => ({ key, node: <Icon /> }));

export function FilterButtonGlyph({ glyph }: { glyph: FilterGlyph }) {
  return <CrossfadeStack class="crossfade-glyph" layers={FILTER_GLYPH_LAYERS} current={glyph} />;
}

export function ThreadFilterButton({ class: extraClass, tooltip }: { class?: string; tooltip?: string }) {
  const open = threadFilterPanelOpen.value;
  const shown = drawerGrouping.value === 'folders';
  const { glyph, pressed } = filterButtonState({
    panelOpen: open,
    channelFilterActive: threadFilterActive.value,
  });
  return (
    <span class="filter-slot" data-shown={shown ? '' : undefined} inert={!shown}>
      <ToggleIconButton
        class={`header-icon filter-btn${extraClass ? ` ${extraClass}` : ''}`}
        pressed={pressed}
        onClick={toggleThreadFilterPanel}
        aria-label="Filter threads"
        aria-expanded={open}
        data-tooltip={tooltip}
      >
        <FilterButtonGlyph glyph={glyph} />
      </ToggleIconButton>
    </span>
  );
}

export const GROUPING_LABEL: Record<DrawerGrouping, string> = { folders: 'Folders', ongoing: 'Ongoing' };

const GROUPING_GLYPH_LAYERS = [
  { key: 'folders', node: <FolderIcon /> },
  { key: 'ongoing', node: <InboxIcon /> },
];

export function otherGrouping(grouping: DrawerGrouping): DrawerGrouping {
  return grouping === 'folders' ? 'ongoing' : 'folders';
}

export function groupingButtonBadgeCount(grouping: DrawerGrouping, blocked: number): number {
  return grouping === 'folders' ? blocked : 0;
}

export function groupingButtonLabel(grouping: DrawerGrouping, badgeCount: number): string {
  return labelWithBlockedCount(`Show ${GROUPING_LABEL[otherGrouping(grouping)]}`, badgeCount);
}

function switchGrouping(next: DrawerGrouping, badgeCount: number): void {
  if (next === 'ongoing') {
    setSelectedOngoingGroup(ongoingGroupOnSwitch(selectedOngoingGroup.value, ongoingGroupLists.value, badgeCount > 0));
  }
  setDrawerGrouping(next);
}

export function ThreadGroupingButton({ class: extraClass }: { class?: string }) {
  const grouping = drawerGrouping.value;
  const next = otherGrouping(grouping);
  const badgeCount = groupingButtonBadgeCount(grouping, blockedThreadCount.value);
  const label = groupingButtonLabel(grouping, badgeCount);
  return (
    <button
      class={`icon-btn header-icon grouping-btn${extraClass ? ` ${extraClass}` : ''}`}
      onClick={() => switchGrouping(next, badgeCount)}
      aria-label={label}
      data-tooltip={label}
    >
      <CrossfadeStack class="crossfade-glyph" layers={GROUPING_GLYPH_LAYERS} current={next} />
      {badgeCount > 0 && <GlyphBadge class="badge" aria-hidden="true">{badgeCount}</GlyphBadge>}
    </button>
  );
}

export function titleFits(scrollWidth: number, clientWidth: number): boolean {
  return scrollWidth <= clientWidth + 0.5;
}

export function ThreadsPaneTitle({ class: className }: { class: string }) {
  const text = threadFilterPanelOpen.value ? 'Filters' : GROUPING_LABEL[drawerGrouping.value];
  const ref = useRef<HTMLSpanElement>(null);
  const [fits, setFits] = useState(true);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const measure = () => setFits(titleFits(el.scrollWidth, el.clientWidth));
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    document.fonts?.addEventListener('loadingdone', measure);
    return () => {
      observer.disconnect();
      document.fonts?.removeEventListener('loadingdone', measure);
    };
  }, [text]);
  return <span ref={ref} class={className} data-cramped={fits ? undefined : ''}>{text}</span>;
}
