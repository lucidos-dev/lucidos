import { signal } from '@preact/signals';
import { getTreeBackfill, raiseTreeBackfillCap } from '../../api/client';
import { showToast } from '../store';
import type { BackfillProgress, TreeBackfill } from '../../api/types';
import { gapRefreshedVersion } from '../eventGap';
import { toFailed, type Loadable } from '../types';

export const treeBackfill = signal<Loadable<TreeBackfill>>({ status: 'not-loaded' });

export const treeBackfillVersion = gapRefreshedVersion();

export const NO_BACKFILL_PROGRESS: BackfillProgress = {
  done: 0,
  total: 0,
  done_milli: 0,
  nodes_done: 0,
  nodes_total: 0,
  waiting_for_model: false,
  retrying: false,
  ready: false,
  spent_usd: null,
  cap_usd: null,
  next_cap_usd: null,
  held: false,
  failing_model: null,
  seq: 0,
};

let framesApplied = 0;

let shownSeq: number | null = null;

let orderResets = 0;

let readsStarted = 0;
let newestReadShown = 0;

function show(data: TreeBackfill, takeOrder = true): void {
  treeBackfill.value = { status: 'loaded', data };
  const progress = data.state === 'running' ? data.progress : data.state === 'ready' ? data.filling : undefined;
  if (progress && takeOrder) shownSeq = progress.seq;
}

export async function loadTreeBackfill(): Promise<void> {
  const before = framesApplied;
  const resetsBefore = orderResets;
  const read = ++readsStarted;
  if (treeBackfill.value.status !== 'loaded') treeBackfill.value = { status: 'loading' };
  try {
    const data = await getTreeBackfill();
    if (before === framesApplied && read > newestReadShown) {
      newestReadShown = read;
      show(data, resetsBefore === orderResets);
    }
  } catch (e) {
    if (before === framesApplied && treeBackfill.value.status !== 'loaded') {
      treeBackfill.value = toFailed(e);
    }
  }
}

export function applyTreeBackfillFrame(next: TreeBackfill): void {
  framesApplied += 1;
  show(next);
}

export function applyTreeBackfillProgress(progress: BackfillProgress): void {
  if (shownSeq !== null && progress.seq <= shownSeq) return;
  applyTreeBackfillFrame(progress.ready ? { state: 'ready', filling: progress } : { state: 'running', progress });
}

export function forgetTreeBackfillOrder(): void {
  shownSeq = null;
  orderResets += 1;
}

export function readyAfterCompleted(): TreeBackfill {
  const current = treeBackfill.value;
  if (current.status !== 'loaded') return { state: 'ready' };
  const state = current.data;
  if (state.state === 'ready') return state;
  return state.state === 'running'
    ? { state: 'ready', filling: { ...state.progress, ready: true } }
    : { state: 'ready' };
}

export function backfillPercent(progress: BackfillProgress): number {
  return progress.total === 0 ? 0 : Math.floor(progress.done_milli / (10 * progress.total));
}

export function backfillDetail(progress: BackfillProgress): string {
  const trees = `${progress.done.toLocaleString()} of ${progress.total.toLocaleString()} trees`;
  if (progress.nodes_total === 0) return trees;
  const nodes = `${progress.nodes_done.toLocaleString()} of ${progress.nodes_total.toLocaleString()}`;
  return `${trees} · ${nodes} summaries in progress`;
}

export async function continueTreeBackfill(fromCap: number): Promise<boolean> {
  try {
    await raiseTreeBackfillCap(fromCap);
    return true;
  } catch (e) {
    showToast(`Could not continue building Tree memory: ${e instanceof Error ? e.message : String(e)}`, 'error');
    return false;
  }
}

export function treeBackfillStarted(): boolean {
  const loadable = treeBackfill.value;
  if (loadable.status !== 'loaded') return false;
  return loadable.data.state !== 'off' || loadable.data.started;
}
