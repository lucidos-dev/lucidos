import { signal } from '@preact/signals';
import { getTreeBackfill } from '../../api/client';
import type { BackfillProgress, TreeBackfill } from '../../api/types';
import { toFailed, type Loadable } from '../types';

/** Where the Tree memory module's backfill has got. The Memory page loads the
 *  snapshot; the `TreeBackfill*` SSE frames move it after that. */
export const treeBackfill = signal<Loadable<TreeBackfill>>({ status: 'not-loaded' });

/** A backfill that has counted nothing yet. */
export const NO_BACKFILL_PROGRESS: BackfillProgress = {
  done: 0,
  total: 0,
  done_milli: 0,
  nodes_done: 0,
  nodes_total: 0,
  waiting_for_model: false,
  retrying: false,
  ready: false,
  seq: 0,
};

/** Bumped by every frame, so a snapshot read that started before one cannot
 *  overwrite it when it lands. */
let framesApplied = 0;

/** The order of the count on screen, or `null` before one. A snapshot read can
 *  overtake progress frames still in flight on SSE, and those must not step
 *  the bar back. */
let shownSeq: number | null = null;

/** Bumped by every order reset. A snapshot read that started before one may
 *  carry a restarted engine's old order, so it shows but sets no order. */
let orderResets = 0;

function show(data: TreeBackfill, takeOrder = true): void {
  treeBackfill.value = { status: 'loaded', data };
  const progress = data.state === 'running' ? data.progress : data.state === 'ready' ? data.filling : undefined;
  if (progress && takeOrder) shownSeq = progress.seq;
}

export async function loadTreeBackfill(): Promise<void> {
  const before = framesApplied;
  const resetsBefore = orderResets;
  if (treeBackfill.value.status !== 'loaded') treeBackfill.value = { status: 'loading' };
  try {
    const data = await getTreeBackfill();
    if (before === framesApplied) show(data, resetsBefore === orderResets);
  } catch (e) {
    // A live state stays: SSE is the newer truth, and the read only refreshes it.
    if (before === framesApplied && treeBackfill.value.status !== 'loaded') {
      treeBackfill.value = toFailed(e);
    }
  }
}

/** Apply one `TreeBackfill*` frame. The SSE dispatcher calls only this and
 *  `applyTreeBackfillProgress`, so the freshness counter cannot be bypassed. */
export function applyTreeBackfillFrame(next: TreeBackfill): void {
  framesApplied += 1;
  show(next);
}

/** Apply a `TreeBackfillProgressed` frame: still building, or ready with the
 *  older threads filling in. A frame no newer than the count shown was already
 *  counted, so it is dropped. */
export function applyTreeBackfillProgress(progress: BackfillProgress): void {
  if (shownSeq !== null && progress.seq <= shownSeq) return;
  applyTreeBackfillFrame(progress.ready ? { state: 'ready', filling: progress } : { state: 'running', progress });
}

/** Forget the order of the count shown. A reconnect can follow an engine
 *  restart, whose frames count from one again. */
export function forgetTreeBackfillOrder(): void {
  shownSeq = null;
  orderResets += 1;
}

/** The state a `TreeBackfillCompleted` frame says: ready, keeping the count
 *  of older threads still to fill in. The bar then never jumps to full and
 *  back before the next progress frame. */
export function readyAfterCompleted(): TreeBackfill {
  const current = treeBackfill.value;
  if (current.status !== 'loaded') return { state: 'ready' };
  const state = current.data;
  if (state.state === 'ready') return state;
  return state.state === 'running'
    ? { state: 'ready', filling: { ...state.progress, ready: true } }
    : { state: 'ready' };
}

/** Whole percent done, counting the built share of each tree under way.
 *  Zero until the compactor has counted the trees. */
export function backfillPercent(progress: BackfillProgress): number {
  return progress.total === 0 ? 0 : Math.floor(progress.done_milli / (10 * progress.total));
}

/** The line under the bar: trees done, and the summaries of the trees under
 *  way, which move while one long tree holds the tree count. */
export function backfillDetail(progress: BackfillProgress): string {
  const trees = `${progress.done.toLocaleString()} of ${progress.total.toLocaleString()} trees`;
  if (progress.nodes_total === 0) return trees;
  const nodes = `${progress.nodes_done.toLocaleString()} of ${progress.nodes_total.toLocaleString()}`;
  return `${trees} · ${nodes} summaries in progress`;
}

/** Whether a backfill started before, so choosing Tree again resumes it with
 *  no new confirm. A running or ready state counts: leaving Tree mid-build
 *  sends no frame. Unknown until the read lands, which reads as no. */
export function treeBackfillStarted(): boolean {
  const loadable = treeBackfill.value;
  if (loadable.status !== 'loaded') return false;
  return loadable.data.state !== 'off' || loadable.data.started;
}
