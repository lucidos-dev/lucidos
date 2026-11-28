import { useEffect, useRef, useState } from 'preact/hooks';
import { LOADER_MIN_VISIBLE_MS } from '../../hooks/useDelayedLoading';
import { CheckIcon, ReloadIcon } from '../shared/icons';
import { panelRefreshAvailable, panelRefreshing, panelRefreshSucceeded, runPanelRefresh } from '../../store/panelRefresh';
import { panelOverlay, parseRepoPath, type PanelOverlay } from '../../store/store';
import { renderHeaderAction, type HeaderActionSpec } from './headerActions';
import { tooltipWithShortcut } from '../../store/actions/keybindings';

export const REFRESH_DONE_HOLD_MS = 400;


export type RefreshIndicatorState = 'idle' | 'refreshing' | 'done';

export interface RefreshIndicatorStep {
  afterMs: number;
  state: RefreshIndicatorState;
}

export function refreshEndSteps(landed: boolean, spunMs: number | null): RefreshIndicatorStep[] {
  if (spunMs === null && !landed) return [];
  const spinLeft = Math.max(0, LOADER_MIN_VISIBLE_MS - (spunMs ?? 0));
  if (!landed) return [{ afterMs: spinLeft, state: 'idle' }];
  const spin: RefreshIndicatorStep[] = spunMs === null ? [{ afterMs: 0, state: 'refreshing' }] : [];
  return [
    ...spin,
    { afterMs: spinLeft, state: 'done' },
    { afterMs: spinLeft + REFRESH_DONE_HOLD_MS, state: 'idle' },
  ];
}

export const DIFF_REFRESH_PINNED = 'Diff is fixed to this change';

export function refreshPinnedToDiff(overlay: PanelOverlay): boolean {
  return overlay?.type === 'file-preview' && parseRepoPath(overlay.path)?.mode === 'diff';
}

export function panelRefreshLive(overlay: PanelOverlay, available: boolean): boolean {
  return available && !refreshPinnedToDiff(overlay);
}

export function refreshPanelIfLive(): void {
  if (panelRefreshLive(panelOverlay.value, panelRefreshAvailable.value)) void runPanelRefresh();
}

const LABELS: Record<RefreshIndicatorState, string | undefined> = {
  idle: undefined,
  refreshing: 'Refreshing',
  done: 'Refreshed',
};

function useRefreshIndicatorState(): RefreshIndicatorState {
  const refreshing = panelRefreshing.value;
  const succeeded = panelRefreshSucceeded.value;
  const seen = useRef(succeeded);
  const startedAt = useRef<number | null>(null);
  const [state, setState] = useState<RefreshIndicatorState>('idle');

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

function RefreshGlyphs() {
  return (
    <>
      <ReloadIcon />
      <CheckIcon className="refresh-check" />
    </>
  );
}

export function MobileRefreshIndicator() {
  const state = useRefreshIndicatorState();
  return (
    <span class="mobile-refresh-indicator" data-state={state} role="status" aria-label={LABELS[state]}>
      <RefreshGlyphs />
    </span>
  );
}

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
