import { DraftsIcon, AttentionIcon } from './icons';
import { draftsViewActive, attentionViewActive, attentionThreadCount, toggleDraftsView, toggleAttentionView } from '../../store/store';
import { draftThreadCount } from '../drawer/family-graph';

interface Props {
  /** Desktop adds hover tooltips and the sibling `threads-header-btn` class;
   *  mobile (no native tooltips) omits both. Mirrors `ThreadNav`'s prop. */
  showTooltip?: boolean;
}

/** The needs-attention and drafts alternate-view toggles, rendered with one
 *  shared `.altview-slot` (sitting immediately right of the filter icon) so the
 *  desktop and mobile threads headers can't drift:
 *
 *  - Each toggle is usable only when it has content — `attentionThreadCount > 0`
 *    / `hasDrafts` — or while its own view is active (so the user can toggle
 *    back out instead of being stranded in a view that just emptied).
 *  - Both toggles stay mounted in order (needs-attention first); an empty one
 *    collapses via `.altview-hidden` so the remaining toggle packs to the first
 *    slot — right of the filter, no empty gap before it. The slot reserves a
 *    fixed two-toggle width (see the CSS) so the title doesn't shift as counts
 *    appear/disappear.
 */
export function AltViewToggles({ showTooltip }: Props) {
  const attentionVisible = attentionThreadCount.value > 0 || attentionViewActive.value;
  const draftCount = draftThreadCount.value;
  const draftsVisible = draftCount > 0 || draftsViewActive.value;
  const btnClass = showTooltip ? 'icon-btn header-icon threads-header-btn' : 'icon-btn header-icon';

  return (
    <div class="altview-slot">
      <button
        class={`${btnClass}${attentionViewActive.value ? ' attention-active' : ''}${attentionVisible ? '' : ' altview-hidden'}`}
        onClick={attentionVisible ? toggleAttentionView : undefined}
        disabled={!attentionVisible}
        aria-hidden={!attentionVisible}
        tabIndex={attentionVisible ? undefined : -1}
        aria-label="Toggle needs-attention view"
        {...(showTooltip && attentionVisible ? { 'data-tooltip': 'Needs attention' } : {})}
      >
        <AttentionIcon />
        {attentionThreadCount.value > 0 && (
          <span class="badge">{attentionThreadCount.value}</span>
        )}
      </button>
      <button
        class={`${btnClass}${draftsViewActive.value ? ' drafts-active' : ''}${draftsVisible ? '' : ' altview-hidden'}`}
        onClick={draftsVisible ? toggleDraftsView : undefined}
        disabled={!draftsVisible}
        aria-hidden={!draftsVisible}
        tabIndex={draftsVisible ? undefined : -1}
        aria-label="Toggle drafts view"
        {...(showTooltip && draftsVisible ? { 'data-tooltip': 'Drafts' } : {})}
      >
        <DraftsIcon />
        {draftCount > 0 && (
          <span class="badge">{draftCount}</span>
        )}
      </button>
    </div>
  );
}
