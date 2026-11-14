import { computed, signal } from '@preact/signals';
import { showToast } from './store';
import { errorDetail } from '../utils/errorDetail';

/**
 * The panel refresh contract (`docs/glossary.md`). Whatever part of the open
 * content panel owns fetched data registers a refresh action for it. Each
 * action must
 *
 *  1. settle only once the new data is on screen, or has failed visibly;
 *  2. keep loaded data on screen while it re-reads, never blanking;
 *  3. start a new read, never join a fetch that began before it was asked.
 *
 * A pull on the content pane, a pull inside an app, and the header's Refresh
 * all run them through `runPanelRefresh`, so they behave the same.
 */
export type PanelRefreshAction = () => Promise<unknown>;

interface Registration {
  /** What failed, in a toast: "Couldn't refresh <noun>". */
  noun: string;
  action: PanelRefreshAction;
  /** Resolves when the part that registered unmounts. A read it left running
   *  may never settle, since an unmount cancels it, so this settles for it. */
  gone: Promise<void>;
}

/** Every refresh the open panel registered. Only one content panel is mounted
 *  at a time, so they all belong to it: a page with two editors registers two,
 *  and a refresh runs both. */
const registrations = signal<readonly Registration[]>([]);

/** Whether the open panel has anything to refresh. */
export const panelRefreshAvailable = computed(() => registrations.value.length > 0);

/** True while a refresh runs. The header spins its Refresh icon on it. */
export const panelRefreshing = signal(false);

/** Bumped when a refresh lands every read. The header shows a check on it, so
 *  even a refresh too fast to see spinning says it happened. */
export const panelRefreshSucceeded = signal(0);

/** How far the pull affordance has travelled, in CSS px. 0 at rest. */
export const panelPullTravel = signal(0);

/** Move the pull affordance. It stays at rest while there is nothing to
 *  refresh, or while a refresh already runs and the header shows it. */
export function showPullTravel(travel: number): void {
  const live = panelRefreshAvailable.peek() && !panelRefreshing.peek();
  panelPullTravel.value = live ? travel : 0;
}

/** Register a refresh for part of the open panel. Returns the unregister. */
export function registerPanelRefresh(noun: string, action: PanelRefreshAction): () => void {
  let unmounted!: () => void;
  const gone = new Promise<void>((resolve) => { unmounted = resolve; });
  const entry: Registration = { noun, action, gone };
  registrations.value = [...registrations.value, entry];
  return () => {
    registrations.value = registrations.value.filter((r) => r !== entry);
    unmounted();
  };
}

/** How one piece of a refresh ended, when it did not fail. */
const LANDED = 'landed';
const CUT_SHORT = 'cut-short';

/** Run every refresh the open panel registered, together. A call while one
 *  runs is ignored, so a second pull cannot start overlapping reads. Each
 *  rejection is shown, naming what failed, never dropped. */
export async function runPanelRefresh(): Promise<void> {
  if (panelRefreshing.peek()) return;
  const pieces = registrations.peek();
  if (pieces.length === 0) return;
  panelRefreshing.value = true;
  try {
    const results = await Promise.allSettled(
      pieces.map((p) => Promise.race([p.action().then(() => LANDED), p.gone.then(() => CUT_SHORT)])),
    );
    results.forEach((result, i) => {
      if (result.status === 'rejected') {
        showToast(`Couldn't refresh ${pieces[i].noun}: ${errorDetail(result.reason)}`, 'error');
      }
    });
    if (results.every((r) => r.status === 'fulfilled' && r.value === LANDED)) panelRefreshSucceeded.value++;
  } finally {
    panelRefreshing.value = false;
  }
}
