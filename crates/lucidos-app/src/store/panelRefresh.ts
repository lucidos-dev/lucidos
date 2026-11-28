import { computed, signal } from '@preact/signals';
import { showToast } from './store';
import { errorDetail } from '../utils/errorDetail';

export type PanelRefreshAction = () => Promise<unknown>;

interface Registration {
  noun: string;
  action: PanelRefreshAction;
  gone: Promise<void>;
}

const registrations = signal<readonly Registration[]>([]);

export const panelRefreshAvailable = computed(() => registrations.value.length > 0);

export const panelRefreshing = signal(false);

export const panelRefreshSucceeded = signal(0);

export const panelPullTravel = signal(0);

export function showPullTravel(travel: number): void {
  const live = panelRefreshAvailable.peek() && !panelRefreshing.peek();
  panelPullTravel.value = live ? travel : 0;
}

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

const LANDED = 'landed';
const CUT_SHORT = 'cut-short';

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
