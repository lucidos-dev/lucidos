// @vitest-environment jsdom
/**
 * What the Tree backfill row says. The tree count holds while one long tree
 * builds, so a detail line counts its summaries. A failed summary says it
 * retries, and a missing model wins over a retry, since nothing moves without
 * one.
 */
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import type { BackfillProgress } from '../../../api/types';
import { NO_BACKFILL_PROGRESS, treeBackfill } from '../../../store/actions/treeBackfill';

const LAST_TREE: BackfillProgress = {
  ...NO_BACKFILL_PROGRESS,
  done: 14,
  total: 15,
  done_milli: 14_617,
  nodes_done: 210,
  nodes_total: 340,
};

async function mount(host: HTMLElement, progress: BackfillProgress): Promise<() => void> {
  const { h, render } = await import('preact');
  const { TreeBackfillStatus } = await import('../TreeBackfillStatus');
  treeBackfill.value = { status: 'loaded', data: { state: 'running', progress } };
  render(h(TreeBackfillStatus, { onPickModel: () => {} }), host);
  return () => render(null, host);
}

function labels(host: HTMLElement): string[] {
  return [...host.querySelectorAll('.progress-label')].map((el) => el.textContent ?? '');
}

describe('the Tree backfill row', () => {
  let host: HTMLElement;
  let unmount: () => void = () => {};

  beforeEach(() => {
    host = document.createElement('div');
    document.body.appendChild(host);
  });

  afterEach(() => {
    unmount();
    host.remove();
  });

  it('moves within the last tree and says how far it has got', async () => {
    unmount = await mount(host, LAST_TREE);
    expect(labels(host)).toEqual([
      'Building memory, 97%',
      '14 of 15 trees · 210 of 340 summaries in progress',
    ]);
    expect(host.querySelector('[data-role="tree-backfill"]')?.hasAttribute('data-working')).toBe(true);
  });

  it('says when a failed summary retries', async () => {
    unmount = await mount(host, { ...LAST_TREE, retrying: true });
    expect(labels(host)[0]).toBe('Building memory, 97%. A summary failed, retrying.');
  });

  it('puts a missing model before a retry, and stops looking busy', async () => {
    unmount = await mount(host, { ...LAST_TREE, retrying: true, waiting_for_model: true });
    expect(labels(host)).toEqual(['Waiting for a background model. Pick one']);
    expect(host.querySelector('[data-role="tree-backfill"]')?.hasAttribute('data-working')).toBe(false);
  });
});
