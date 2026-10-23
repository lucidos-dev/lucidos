// @vitest-environment jsdom
/**
 * What the Tree backfill row says. The tree count holds while one long tree
 * builds, so a detail line counts its summaries. A failed summary says it
 * retries, and a missing model wins over a retry, since nothing moves without
 * one. A spend line shows what the backfill has cost, and a backfill held at
 * its cap offers Continue and a switch to Classic.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import type { BackfillProgress } from '../../../api/types';
import { NO_BACKFILL_PROGRESS, treeBackfill } from '../../../store/actions/treeBackfill';

const raiseTreeBackfillCap = vi.fn(async (_fromCap: number) => ({ cap_usd: 0 }));
vi.mock('../../../api/client', async (actual) => ({
  ...(await actual<typeof import('../../../api/client')>()),
  raiseTreeBackfillCap: (fromCap: number) => raiseTreeBackfillCap(fromCap),
}));

const LAST_TREE: BackfillProgress = {
  ...NO_BACKFILL_PROGRESS,
  done: 14,
  total: 15,
  done_milli: 14_617,
  nodes_done: 210,
  nodes_total: 340,
};

async function mount(
  host: HTMLElement,
  progress: BackfillProgress,
  onSwitchToClassic: () => void = () => {},
): Promise<() => void> {
  const { h, render } = await import('preact');
  const { TreeBackfillStatus } = await import('../TreeBackfillStatus');
  treeBackfill.value = { status: 'loaded', data: { state: 'running', progress } };
  render(h(TreeBackfillStatus, { onPickModel: () => {}, onSwitchToClassic }), host);
  return () => render(null, host);
}

function buttonNamed(host: HTMLElement, text: string): HTMLButtonElement {
  const found = [...host.querySelectorAll('button')].find((b) => b.textContent === text);
  if (!found) throw new Error(`no ${text} button`);
  return found;
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

  it('shows no spend for a model with no price', async () => {
    unmount = await mount(host, { ...LAST_TREE, spent_usd: null, cap_usd: null });
    expect(labels(host)).toHaveLength(2);
  });

  it('shows the spend against the cap while it builds', async () => {
    unmount = await mount(host, { ...LAST_TREE, spent_usd: 412.4, cap_usd: 2550 });
    expect(labels(host)).toEqual([
      'Building memory, 97%',
      '14 of 15 trees · 210 of 340 summaries in progress',
      '$412 of $2,550 cap',
    ]);
  });

  it('pauses at the cap, and offers Continue and a switch to Classic', async () => {
    const toClassic = vi.fn();
    const held = { ...LAST_TREE, spent_usd: 2551, cap_usd: 2550, next_cap_usd: 3401, held: true };
    unmount = await mount(host, held, toClassic);
    expect(labels(host)[0]).toBe('Paused at the $2,550 spending cap. Continue to $3,401 or Switch to Classic');
    expect(host.querySelector('[data-role="tree-backfill"]')?.getAttribute('data-state')).toBe('held');
    expect(host.querySelector('[data-role="tree-backfill"]')?.hasAttribute('data-working')).toBe(false);

    buttonNamed(host, 'Continue to $3,401').click();
    expect(raiseTreeBackfillCap).toHaveBeenCalledExactlyOnceWith(2550);
    await new Promise((r) => setTimeout(r, 0));
    expect(buttonNamed(host, 'Continue to $3,401').disabled, 'one press per cap').toBe(true);
    buttonNamed(host, 'Switch to Classic').click();
    expect(toClassic).toHaveBeenCalledOnce();
  });

  it('puts a missing model before a retry, and stops looking busy', async () => {
    unmount = await mount(host, { ...LAST_TREE, retrying: true, waiting_for_model: true });
    expect(labels(host)).toEqual(['Waiting for a background model. Pick one']);
    expect(host.querySelector('[data-role="tree-backfill"]')?.hasAttribute('data-working')).toBe(false);
  });
});
