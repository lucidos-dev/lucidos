import { useEffect, useRef, useState } from 'preact/hooks';
import { CheckIcon, ReloadIcon } from '../shared/icons';
import { panelRefreshAvailable, panelRefreshing, panelRefreshSucceeded, runPanelRefresh } from '../../store/panelRefresh';
import { panelOverlay, parseRepoPath, type PanelOverlay } from '../../store/store';
import { renderHeaderAction, type HeaderActionSpec } from './headerActions';
import { tooltipWithShortcut } from '../../store/actions/keybindings';

/** How long the spinner shows at least, so a quick refresh reads as one before
 *  it turns into the check. Holds only this status icon, never the content. */
export const REFRESH_MIN_SPIN_MS = 400;

/** How long the check stays up after a refresh lands. The fade out starts
 *  when this ends. */
export const REFRESH_DONE_HOLD_MS = 400;

// Both holds are for legibility, not animation, so the speed setting leaves them alone.

export type RefreshIndicatorState = 'idle' | 'refreshing' | 'done';

export interface RefreshIndicatorStep {
  afterMs: number;
  state: RefreshIndicatorState;
}

/** What the indicator shows once a refresh that spun for `spunMs` ends. A
 *  refresh that did not land every read ends without a check. `spunMs` is null
 *  when no render saw the refresh running: a quick one can start and land
 *  between two renders, and still owes its spin and its check. */
export function refreshEndSteps(landed: boolean, spunMs: number | null): RefreshIndicatorStep[] {
  if (spunMs === null && !landed) return [];
  const spinLeft = Math.max(0, REFRESH_MIN_SPIN_MS - (spunMs ?? 0));
  if (!landed) return [{ afterMs: spinLeft, state: 'idle' }];
  const spin: RefreshIndicatorStep[] = spunMs === null ? [{ afterMs: 0, state: 'refreshing' }] : [];
  return [
    ...spin,
    { afterMs: spinLeft, state: 'done' },
    { afterMs: spinLeft + REFRESH_DONE_HOLD_MS, state: 'idle' },
  ];
}

/** Why a diff's Refresh is drawn disabled. */
export const DIFF_REFRESH_PINNED = 'Diff is fixed to this change';

/** Whether the open panel is a diff. A diff is pinned to its change, so its
 *  Refresh is drawn disabled. */
export function refreshPinnedToDiff(overlay: PanelOverlay): boolean {
  return overlay?.type === 'file-preview' && parseRepoPath(overlay.path)?.mode === 'diff';
}

/** Whether the open panel's Refresh can run. The diff is read from the overlay:
 *  the preview drops its registration in an effect, after this render. */
export function panelRefreshLive(overlay: PanelOverlay, available: boolean): boolean {
  return available && !refreshPinnedToDiff(overlay);
}

/** The Refresh shortcut. It runs only where the header's Refresh can, so a
 *  diff stays pinned to its change. */
export function refreshPanelIfLive(): void {
  if (panelRefreshLive(panelOverlay.value, panelRefreshAvailable.value)) void runPanelRefresh();
}

const LABELS: Record<RefreshIndicatorState, string | undefined> = {
  idle: undefined,
  refreshing: 'Refreshing',
  done: 'Refreshed',
};

/** Spinning while a panel refresh runs, then `done` once every read has
 *  landed. Each end holds for its minimum, see `refreshEndSteps`. */
function useRefreshIndicatorState(): RefreshIndicatorState {
  const refreshing = panelRefreshing.value;
  const succeeded = panelRefreshSucceeded.value;
  // The count at mount is history: only a refresh landing from now on checks.
  const seen = useRef(succeeded);
  // When the running refresh started, or null between refreshes.
  const startedAt = useRef<number | null>(null);
  const [state, setState] = useState<RefreshIndicatorState>('idle');

  // A new refresh takes over from whatever is on screen, and clears its timers.
  useEffect(() => {
    if (refreshing) {
      startedAt.current ??= performance.now();
      setState('refreshing');
      return;
    }
    const landed = succeeded !== seen.current;
    seen.current = succeeded;
    const spunMs = startedAt.current === null ? null : performance.now() - startedAt.current;
    startedAt.current = null;
    const timers = refreshEndSteps(landed, spunMs).map((step) => setTimeout(() => setState(step.state), step.afterMs));
    return () => timers.forEach(clearTimeout);
  }, [refreshing, succeeded]);

  return state;
}

/** The arrow and the check share one cell, and the CSS crossfades them. */
function RefreshGlyphs() {
  return (
    <>
      <ReloadIcon />
      <CheckIcon className="refresh-check" />
    </>
  );
}

/** A panel refresh in the slot beside the hamburger: a spinner while it runs,
 *  then a check once every read has landed. The edge reserve holds that slot
 *  empty on this row, so nothing moves when it appears. The slot beside the
 *  bell belongs to the ⋯ trigger, and a spinner there would fold away with
 *  the actions. */
export function MobileRefreshIndicator() {
  const state = useRefreshIndicatorState();
  return (
    <span class="mobile-refresh-indicator" data-state={state} role="status" aria-label={LABELS[state]}>
      <RefreshGlyphs />
    </span>
  );
}

/** The desktop content row's Refresh, beside the hamburger. The button is its
 *  own indicator: the arrow spins for at least the minimum, turns into the
 *  check, then comes back as the arrow. Leading the row keeps it out of the
 *  trailing cluster's fold. Until it is the arrow again it takes no press,
 *  but keeps its tooltip, so it is `aria-disabled` rather than `disabled`. */
export function ContentRefreshButton() {
  const state = useRefreshIndicatorState();
  const overlay = panelOverlay.value;
  const spec: HeaderActionSpec = {
    key: 'refresh',
    label: LABELS[state] ?? 'Refresh',
    tooltip: tooltipWithShortcut('Refresh', 'refreshPanel'),
    icon: () => <RefreshGlyphs />,
    extraClass: 'content-refresh-btn',
  };
  if (refreshPinnedToDiff(overlay)) {
    return renderHeaderAction({ ...spec, disabledTooltip: DIFF_REFRESH_PINNED }, { 'data-state': 'idle' });
  }
  if (!panelRefreshLive(overlay, panelRefreshAvailable.value)) return null;
  const busy = state !== 'idle';
  return renderHeaderAction(
    { ...spec, onClick: busy ? undefined : () => void runPanelRefresh() },
    { 'data-state': state, 'aria-disabled': String(busy) },
  );
}
