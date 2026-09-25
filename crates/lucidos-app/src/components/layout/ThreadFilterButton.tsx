import { useRef } from 'preact/hooks';
import { CrossfadeStack } from '../shared/CrossfadeStack';
import { FILTER_BUTTON_GLYPHS, filterButtonState, type FilterGlyph } from './ThreadFilterPanel';
import { threadFilterPanelOpen, toggleThreadFilterPanel } from '../../store/threadFilterPanel';
import { threadFilterActive } from '../../store/threadFilterActive';
import { drawerView, attentionThreadCount } from '../../store/store';

/** The glyph layers never change, so they are built once. */
const GLYPH_LAYERS = Object.entries(FILTER_BUTTON_GLYPHS).map(([key, Icon]) => ({ key, node: <Icon /> }));

/** The threads-pane title's two words, which crossfade as the filter panel
 *  opens and closes. */
export const THREADS_TITLE_LAYERS = [
  { key: 'threads', node: 'Threads' },
  { key: 'filters', node: 'Filters' },
] as const;

/** The Filter button's glyph: every glyph it can wear, crossfading to `glyph`.
 *
 *  The header paints resting icons in a translucent white, and two translucent
 *  shapes stacked mid-fade paint their overlap twice. So `.filter-glyph` takes
 *  the translucency as its own `opacity` and paints its shapes opaque
 *  (panels/shell.css). The group is then composited once. */
export function FilterButtonGlyph({ glyph }: { glyph: FilterGlyph }) {
  return <CrossfadeStack class="filter-glyph" layers={GLYPH_LAYERS} current={glyph} />;
}

/** The needs-attention badge, always mounted so it can fade both ways.
 *
 *  It fades out when the count drops to 0, so it keeps drawing the last count
 *  it showed until the fade ends. A count change while shown just repaints the
 *  number. The button's `aria-label` names it, so the badge stays out of the
 *  accessibility tree. */
export function FilterButtonBadge({ count }: { count: number }) {
  const lastShown = useRef(count);
  if (count > 0) lastShown.current = count;
  return (
    <span class="badge filter-badge" data-shown={count > 0 ? '' : undefined} aria-hidden="true">
      {lastShown.current > 0 ? lastShown.current : ''}
    </span>
  );
}

/** The threads-header Filter button, on both layouts. Its whole look comes
 *  from `filterButtonState`.
 *
 *  It toggles a panel that renders down in the drawer pane (`ThreadDrawer`),
 *  and it is that panel's only way out. The accessible NAME stays "Filter
 *  threads" either way (the disclosure pattern): `aria-expanded` says which way
 *  the next press goes. */
export function ThreadFilterButton({ class: extraClass, tooltip }: { class?: string; tooltip?: string }) {
  const open = threadFilterPanelOpen.value;
  const { glyph, pressed, badge } = filterButtonState({
    view: drawerView.value,
    panelOpen: open,
    channelFilterActive: threadFilterActive.value,
    attentionCount: attentionThreadCount.value,
  });
  const classes = ['icon-btn', 'header-icon', 'filter-btn'];
  if (extraClass) classes.push(extraClass);
  if (pressed) classes.push('view-selector-active');
  return (
    <button
      class={classes.join(' ')}
      onClick={toggleThreadFilterPanel}
      aria-label="Filter threads"
      aria-expanded={open}
      data-tooltip={tooltip}
    >
      <FilterButtonGlyph glyph={glyph} />
      <FilterButtonBadge count={badge} />
    </button>
  );
}

/** The threads-pane title, on both layouts. It says what the pane shows: the
 *  list, or the filter panel covering it, which carries no title row of its
 *  own. Just "Filters": the pane is already the Threads pane. */
export function ThreadsPaneTitle({ class: className }: { class: string }) {
  return (
    <CrossfadeStack
      class={className}
      layers={THREADS_TITLE_LAYERS}
      current={threadFilterPanelOpen.value ? 'filters' : 'threads'}
    />
  );
}
