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
};

/** Bumped by every frame, so a snapshot read that started before one cannot
 *  overwrite it when it lands. */
let framesApplied = 0;

export async function loadTreeBackfill(): Promise<void> {
  const before = framesApplied;
  if (treeBackfill.value.status !== 'loaded') treeBackfill.value = { status: 'loading' };
  try {
    const data = await getTreeBackfill();
    if (before === framesApplied) treeBackfill.value = { status: 'loaded', data };
  } catch (e) {
    // A live state stays: SSE is the newer truth, and the read only refreshes it.
    if (before === framesApplied && treeBackfill.value.status !== 'loaded') {
      treeBackfill.value = toFailed(e);
    }
  }
}

/** Apply one `TreeBackfill*` frame. The single entry point the SSE dispatcher
 *  calls, so the freshness counter cannot be bypassed. */
export function applyTreeBackfillFrame(next: TreeBackfill): void {
  framesApplied += 1;
  treeBackfill.value = { status: 'loaded', data: next };
}

/** The state a `TreeBackfillProgressed` frame says: still building, or ready
 *  with the older threads filling in. */
export function treeBackfillOfProgress(progress: BackfillProgress): TreeBackfill {
  return progress.ready ? { state: 'ready', filling: progress } : { state: 'running', progress };
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
