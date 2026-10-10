import { useLayoutEffect, useRef, useState } from 'preact/hooks';
import { CrossfadeStack } from '../shared/CrossfadeStack';
import { GlyphBadge } from '../shared/GlyphBadge';
import { labelWithBlockedCount } from '../shared/ThreadToggleButton';
import { FolderIcon, InboxIcon } from '../shared/icons';
import { FILTER_BUTTON_GLYPHS, filterButtonState, type FilterGlyph } from './ThreadFilterPanel';
import { threadFilterPanelOpen, toggleThreadFilterPanel } from '../../store/threadFilterPanel';
import { threadFilterActive } from '../../store/threadFilterActive';
import { blockedThreadCount, drawerGrouping, setDrawerGrouping, selectedOngoingGroup, setSelectedOngoingGroup, type DrawerGrouping } from '../../store/store';
import { ongoingGroupLists, ongoingGroupOnSwitch } from '../drawer/family-graph';

/** The glyph layers never change, so they are built once. */
const FILTER_GLYPH_LAYERS = Object.entries(FILTER_BUTTON_GLYPHS).map(([key, Icon]) => ({ key, node: <Icon /> }));

/** The Filter button's glyph: every glyph it can wear, crossfading to `glyph`.
 *
 *  The header paints resting icons in a translucent white, and two translucent
 *  shapes stacked mid-fade paint their overlap twice. So `.crossfade-glyph`
 *  takes the translucency as its own `opacity` and paints its shapes opaque
 *  (panels/shell.css). The group is then composited once. */
export function FilterButtonGlyph({ glyph }: { glyph: FilterGlyph }) {
  return <CrossfadeStack class="crossfade-glyph" layers={FILTER_GLYPH_LAYERS} current={glyph} />;
}

/** The threads-header Filter button, on both layouts. Its whole look comes
 *  from `filterButtonState`.
 *
 *  It toggles a panel that renders down in the drawer pane (`ThreadDrawer`),
 *  and it is that panel's visible way out. The accessible NAME stays "Filter
 *  threads" either way (the disclosure pattern): `aria-expanded` says which way
 *  the next press goes.
 *
 *  The filter shapes the Folders grouping only, so the button shows only
 *  there. Under Ongoing it fades out in a slot that keeps its box
 *  (`.filter-slot` in panels/shell.css), so the grouping button beside it never
 *  moves. `inert` takes it out of the tab order and the accessibility tree. */
export function ThreadFilterButton({ class: extraClass, tooltip }: { class?: string; tooltip?: string }) {
  const open = threadFilterPanelOpen.value;
  const shown = drawerGrouping.value === 'folders';
  const { glyph, pressed } = filterButtonState({
    panelOpen: open,
    channelFilterActive: threadFilterActive.value,
  });
  const classes = ['icon-btn', 'header-icon', 'filter-btn'];
  if (extraClass) classes.push(extraClass);
  if (pressed) classes.push('view-selector-active');
  return (
    <span class="filter-slot" data-shown={shown ? '' : undefined} inert={!shown}>
      <button
        class={classes.join(' ')}
        onClick={toggleThreadFilterPanel}
        aria-label="Filter threads"
        aria-expanded={open}
        data-tooltip={tooltip}
      >
        <FilterButtonGlyph glyph={glyph} />
      </button>
    </span>
  );
}

/** What each grouping is called, in the pane title and the button's label. */
export const GROUPING_LABEL: Record<DrawerGrouping, string> = { folders: 'Folders', ongoing: 'Ongoing' };

const GROUPING_GLYPH_LAYERS = [
  { key: 'folders', node: <FolderIcon /> },
  { key: 'ongoing', node: <InboxIcon /> },
];

/** The grouping a press on the grouping button switches to. */
export function otherGrouping(grouping: DrawerGrouping): DrawerGrouping {
  return grouping === 'folders' ? 'ongoing' : 'folders';
}

/** The Blocked count the grouping button badges. Only under Folders:
 *  there the button shows Ongoing, where those threads are. Under Ongoing the
 *  Blocked tile carries the count. */
export function groupingButtonBadgeCount(grouping: DrawerGrouping, blocked: number): number {
  return grouping === 'folders' ? blocked : 0;
}

/** The button's name says what a press does, and carries the badge's count,
 *  which is decorative markup and invisible to assistive tech otherwise. */
export function groupingButtonLabel(grouping: DrawerGrouping, badgeCount: number): string {
  return labelWithBlockedCount(`Show ${GROUPING_LABEL[otherGrouping(grouping)]}`, badgeCount);
}

/** Switch to `next`. A switch to Ongoing first selects the group the user most
 *  likely came for (`ongoingGroupOnSwitch`). */
function switchGrouping(next: DrawerGrouping, badgeCount: number): void {
  if (next === 'ongoing') {
    setSelectedOngoingGroup(ongoingGroupOnSwitch(selectedOngoingGroup.value, ongoingGroupLists.value, badgeCount > 0));
  }
  setDrawerGrouping(next);
}

/** The threads-header grouping button, on both layouts. One slot for two
 *  groupings: it shows the grouping a press goes to, since the pane title
 *  already names the one on screen. */
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

/** Whether a title's text fits its box whole, with a sub-pixel fudge. */
export function titleFits(scrollWidth: number, clientWidth: number): boolean {
  return scrollWidth <= clientWidth + 0.5;
}

/** The threads-pane title, on both layouts. It names what the pane shows: the
 *  grouping, or "Filters" while the filter panel covers the list. It switches
 *  word at once: only the view below it moves.
 *
 *  It is whole or hidden, never an ellipsis. The row clamps it to the room
 *  between the two ends. A word that outgrows that room fades out
 *  (`data-cramped`, panels/shell.css). */
export function ThreadsPaneTitle({ class: className }: { class: string }) {
  const text = threadFilterPanelOpen.value ? 'Filters' : GROUPING_LABEL[drawerGrouping.value];
  const ref = useRef<HTMLSpanElement>(null);
  const [fits, setFits] = useState(true);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const measure = () => setFits(titleFits(el.scrollWidth, el.clientWidth));
    measure();
    // The box changes width when the row does, and when the word does. A font
    // load can narrow the word inside a clamped box that keeps its width.
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
