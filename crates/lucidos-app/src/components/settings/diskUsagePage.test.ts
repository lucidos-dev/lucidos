/**
 * Disk Usage recovers from a failed read.
 *
 * Its loadables are module-level signals, and the page used to read only from
 * `not-loaded`. One failed read then showed the error on every later visit,
 * with no Retry, until the whole app reloaded.
 */
import { afterEach, describe, it, expect, vi } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import {
  cleanupProgressLine,
  describeEstimate,
  diskUsageLoadOwed,
  estimateRecommendedCleanup,
  removeUnlessDirty,
  runCleanup,
  type WorktreeRow,
} from './DiskUsagePage';

/** Answer every cleanup POST with one status and a JSON body. */
function answerCleanup(status: number, body: unknown) {
  vi.stubGlobal('fetch', async () => new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  }));
}

const LIVE_SESSION = { error: 'Thread has a live coding-agent session in this worktree.' };

describe('a cleanup the engine refuses', () => {
  afterEach(() => { vi.unstubAllGlobals(); });

  // The engine answers 409 on EVERY tier while a coding-agent session is live.
  // Read as "dirty", Clean reported "Nothing to clean", and a forced Remove
  // ended in silence after the user had confirmed losing their edits.
  it('surfaces the reason on Clean (tier 1)', async () => {
    answerCleanup(409, LIVE_SESSION);
    await expect(runCleanup('t', 1)).rejects.toThrow(/live coding-agent session/);
  });

  it('surfaces the reason on a forced Remove (tier 3)', async () => {
    answerCleanup(409, LIVE_SESSION);
    await expect(runCleanup('t', 3)).rejects.toThrow(/live coding-agent session/);
  });

  it('reads a tier-2 409 as the cue to offer tier 3', async () => {
    answerCleanup(409, { error: 'Worktree has uncommitted changes' });
    await expect(removeUnlessDirty('t')).resolves.toBeNull();
  });

  it('still throws any other tier-2 failure', async () => {
    answerCleanup(500, { error: 'disk on fire' });
    await expect(removeUnlessDirty('t')).rejects.toThrow(/disk on fire/);
  });

  it('hands back what a tier-2 removal freed', async () => {
    answerCleanup(200, { tier: 2, freed_bytes: 42, branch_deleted: true });
    await expect(removeUnlessDirty('t')).resolves.toEqual({ tier: 2, freed_bytes: 42, branch_deleted: true });
  });
});

describe('diskUsageLoadOwed', () => {
  it('reads on a first visit', () => {
    expect(diskUsageLoadOwed({ status: 'not-loaded' })).toBe(true);
  });

  it('reads again after a failure', () => {
    expect(diskUsageLoadOwed({ status: 'failed', error: 'engine restarting' })).toBe(true);
  });

  it('leaves a read in flight alone', () => {
    expect(diskUsageLoadOwed({ status: 'loading' })).toBe(false);
  });

  it('keeps a loaded page as it is', () => {
    expect(diskUsageLoadOwed({ status: 'loaded', data: [] })).toBe(false);
  });
});

describe('the failed page', () => {
  it('offers a Retry that reads the inventory again', () => {
    const here: string = dirname(fileURLToPath(import.meta.url));
    const source: string = readFileSync(resolve(here, 'DiskUsagePage.tsx'), 'utf-8');
    expect(source).toContain('onRetry={() => void loadInventory()}');
  });
});

describe('the recommended cleanup estimate', () => {
  const row = (over: Partial<WorktreeRow>): WorktreeRow => ({
    thread_id: 't',
    thread_title: null,
    worktree_path: '/tmp/wt',
    size_bytes: 1000,
    artifact_bytes: 600,
    last_activity: null,
    is_dirty: false,
    is_saved: false,
    is_active: false,
    is_finished: false,
    ...over,
  });

  it('counts a finished tree whole and every other one by its artifacts', () => {
    expect(estimateRecommendedCleanup([
      row({ is_finished: true }),
      row({}),
      row({ is_dirty: true, artifact_bytes: 50 }),
    ])).toEqual({ removeCount: 1, cleanCount: 2, bytes: 1650 });
  });

  it('skips live and pinned trees, as the engine does', () => {
    expect(estimateRecommendedCleanup([
      row({ is_finished: true, is_active: true }),
      row({ is_finished: true, is_saved: true }),
      row({ is_saved: true }),
    ])).toEqual({ removeCount: 0, cleanCount: 0, bytes: 0 });
  });

  it('counts no tree with nothing to strip', () => {
    expect(estimateRecommendedCleanup([row({ artifact_bytes: 0 })]).cleanCount).toBe(0);
  });

  it('names only the parts that will run', () => {
    expect(describeEstimate({ removeCount: 2, cleanCount: 0, bytes: 1 }))
      .toMatch(/^Removes 2 finished worktrees\. /);
    expect(describeEstimate({ removeCount: 1, cleanCount: 3, bytes: 1 }))
      .toMatch(/^Removes 1 finished worktree and clears build artifacts in 3 worktrees\. /);
  });
});

describe('the running cleanup line', () => {
  it('says only that a pass runs until its count is known', () => {
    expect(cleanupProgressLine({ done: 0, total: 0 })).toBe('Cleanup in progress');
  });

  it('counts the worktrees dealt with so far', () => {
    expect(cleanupProgressLine({ done: 12, total: 105 })).toBe('Cleanup in progress: 12 of 105 worktrees');
    expect(cleanupProgressLine({ done: 0, total: 1 })).toBe('Cleanup in progress: 0 of 1 worktree');
  });
});
