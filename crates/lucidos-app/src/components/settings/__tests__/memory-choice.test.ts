// @vitest-environment jsdom
/**
 * The memory choice asks a workspace that never chose its memory module. It
 * leads with what Tree costs a month for three ways of using a workspace.
 * Every way out writes the preference: Use Classic and closing write
 * `classic`, and Use Tree writes `tree` once the estimate has loaded.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import type { BackgroundModels, TreeBackfillEstimate } from '../../../api/types';

const {
  getTreeBackfillEstimate, getBackgroundModels, setMemoryModule, loadTreeBackfill,
} = vi.hoisted(() => ({
  getTreeBackfillEstimate: vi.fn<() => Promise<TreeBackfillEstimate>>(),
  getBackgroundModels: vi.fn<() => Promise<BackgroundModels>>(),
  setMemoryModule: vi.fn<(m: string) => Promise<void>>(),
  loadTreeBackfill: vi.fn<() => Promise<void>>(),
}));

vi.mock('../../../api/client', async () => {
  const actual = await vi.importActual<Record<string, unknown>>('../../../api/client');
  return { ...actual, getTreeBackfillEstimate, getBackgroundModels };
});

vi.mock('../../../store/actions/preferences', async () => {
  const actual = await vi.importActual<Record<string, unknown>>('../../../store/actions/preferences');
  return { ...actual, setMemoryModule };
});

vi.mock('../../../store/actions/treeBackfill', async () => {
  const actual = await vi.importActual<Record<string, unknown>>('../../../store/actions/treeBackfill');
  return { ...actual, loadTreeBackfill };
});

import { stubIntentFieldObservers } from '../../triggers/__tests__/intentFieldStubs';

stubIntentFieldObservers();

const SOL_LOW = {
  effort: 'low',
  usable_secs: { low: 0, high: 0 },
  complete_secs: { low: 0, high: 0 },
  backfill_usd: { low: 0, high: 0 },
  backfill_usd_central: 0,
  daily_usd: { low: 0, high: 0 },
  daily_usd_central: 0,
  turn_usd: { low: 0.04, high: 0.09 },
  turn_usd_central: 0.06,
  cap_usd: 0,
};

/** A new workspace: no history, so only the per-turn price says anything. */
const FRESH: TreeBackfillEstimate = {
  calls: { low: 0, high: 0 },
  usable_secs: { low: 0, high: 0 },
  complete_secs: { low: 0, high: 0 },
  daily_calls: { low: 0, high: 0 },
  turn_calls: { low: 7.6, high: 10.9 },
  costs: [{ model: 'gpt-6.1-sol', by_effort: [SOL_LOW] }],
};

/** A workspace from before, unused until now but with threads to summarize. */
const WITH_HISTORY: TreeBackfillEstimate = {
  ...FRESH,
  calls: { low: 210_000, high: 260_000 },
  usable_secs: { low: 20 * 60, high: 80 * 60 },
  complete_secs: { low: 6 * 3600, high: 19 * 3600 },
  costs: [{
    model: 'gpt-6.1-sol',
    by_effort: [{ ...SOL_LOW, backfill_usd: { low: 1100, high: 2300 }, backfill_usd_central: 1700, cap_usd: 2550 }],
  }],
};

async function mount(host: HTMLElement): Promise<() => void> {
  const { h, render } = await import('preact');
  const store = await import('../../../store/store');
  const { MemoryChoice } = await import('../MemoryChoice');
  store.preferences.value = { status: 'loaded', data: {} };
  render(h(MemoryChoice, {}), host);
  return () => render(null, host);
}

function button(host: HTMLElement, name: string): HTMLButtonElement {
  const found = [...host.querySelectorAll('button')].find((b) => b.textContent?.trim() === name);
  if (!found) throw new Error(`no "${name}" button`);
  return found;
}

function tiles(host: HTMLElement): string[] {
  return [...host.querySelectorAll('[data-role="memory-choice-tile"]')].map((t) => t.textContent ?? '');
}

/** Preact runs effects after paint, which jsdom only reaches through the
 *  100 ms fallback timer, so a settle waits past it. */
async function settle(): Promise<void> {
  await new Promise((r) => setTimeout(r, 150));
}

describe('the memory choice', () => {
  let host: HTMLElement;
  let unmount: () => void = () => {};

  beforeEach(() => {
    host = document.createElement('div');
    document.body.appendChild(host);
    getTreeBackfillEstimate.mockReset();
    getBackgroundModels.mockReset().mockResolvedValue({
      model_summary_compaction: {
        model: 'gpt-6.1-sol',
        effort: 'low',
        source: 'default',
        reachable: true,
        not_served: [],
        needs_vision: false,
        asks_no_model: false,
        vision: true,
        recommended: [{ model: 'gpt-6.1-sol', effort: 'low' }],
      },
    });
    setMemoryModule.mockReset().mockResolvedValue(undefined);
    loadTreeBackfill.mockReset().mockResolvedValue(undefined);
  });

  afterEach(() => {
    unmount();
    host.remove();
  });

  it('asks as a modal dialog, with Classic almost free and Tree priced', async () => {
    getTreeBackfillEstimate.mockResolvedValue(FRESH);
    unmount = await mount(host);
    await settle();
    const dialog = host.querySelector('[data-role="memory-choice"]');
    expect(dialog?.getAttribute('role')).toBe('dialog');
    expect(dialog?.getAttribute('aria-modal')).toBe('true');
    expect(dialog?.closest('.modal-overlay')).not.toBeNull();
    const text = dialog?.textContent ?? '';
    expect(text).toContain('Choose how this workspace remembers');
    expect(text).toContain('Almost free');
    expect(text).toContain('Costs money');
    expect(text).toContain('If you close this, the workspace stays on Classic.');
  });

  it('leads with a month for each profile, from the price of one turn', async () => {
    getTreeBackfillEstimate.mockResolvedValue(FRESH);
    unmount = await mount(host);
    await settle();
    const [light, medium, heavy] = tiles(host);
    expect(light).toContain('Light');
    expect(light).toContain('$3 a month');
    expect(light).toContain('$2 to $5');
    expect(light).toContain('About 10 chats a month');
    expect(medium).toContain('$15 a month');
    expect(medium).toContain('20 coding threads and some chats a month');
    expect(heavy).toContain('$90 a month');
    expect(heavy).toContain('$60 to $135');
    expect(heavy).toContain('200 coding threads a month');
    expect(host.textContent).toContain('About $0.06 a turn.');
  });

  /** A new workspace has nothing to summarize, so $0 figures would mislead. */
  it('shows the one-time summary only for a workspace with history', async () => {
    getTreeBackfillEstimate.mockResolvedValue(FRESH);
    unmount = await mount(host);
    await settle();
    expect(host.querySelector('[data-role="tree-estimate"]')).toBeNull();
    unmount();

    getTreeBackfillEstimate.mockResolvedValue(WITH_HISTORY);
    unmount = await mount(host);
    await settle();
    const estimate = host.querySelector('[data-role="tree-estimate"]')?.textContent ?? '';
    expect(estimate).toContain('$1,100 to $2,300');
    expect(estimate).toContain('Pauses at$2,550');
  });

  it('holds Use Tree until the estimate loads, then starts Tree', async () => {
    let resolve: (e: TreeBackfillEstimate) => void = () => {};
    getTreeBackfillEstimate.mockReturnValue(new Promise((r) => { resolve = r; }));
    unmount = await mount(host);
    await settle();
    expect(button(host, 'Use Tree').disabled).toBe(true);

    resolve(FRESH);
    await settle();
    expect(button(host, 'Use Tree').disabled).toBe(false);
    button(host, 'Use Tree').click();
    await settle();
    expect(setMemoryModule).toHaveBeenCalledExactlyOnceWith('tree');
    expect(loadTreeBackfill).toHaveBeenCalledOnce();
  });

  it('writes Classic on Use Classic', async () => {
    getTreeBackfillEstimate.mockResolvedValue(FRESH);
    unmount = await mount(host);
    await settle();
    button(host, 'Use Classic').click();
    expect(setMemoryModule).toHaveBeenCalledExactlyOnceWith('classic');
  });

  it('writes Classic when closed, so it never asks again', async () => {
    getTreeBackfillEstimate.mockResolvedValue(FRESH);
    unmount = await mount(host);
    await settle();
    host.querySelector<HTMLButtonElement>('[data-role="surface-close"]')!.click();
    expect(setMemoryModule).toHaveBeenCalledExactlyOnceWith('classic');
  });

  it('still offers Classic when the estimate fails, and Try again reads it again', async () => {
    getTreeBackfillEstimate
      .mockRejectedValueOnce(new Error('Request timed out after 60000ms'))
      .mockResolvedValueOnce(FRESH);
    unmount = await mount(host);
    await settle();
    expect(host.textContent).toContain('Could not estimate the cost: Request timed out after 60000ms');
    expect(tiles(host)).toEqual([]);
    expect(button(host, 'Use Tree').disabled).toBe(true);
    expect(button(host, 'Use Classic').disabled).toBe(false);

    button(host, 'Try again').click();
    await settle();
    expect(tiles(host)[0]).toContain('$3 a month');
    expect(button(host, 'Use Tree').disabled).toBe(false);
  });

  it('says a model it cannot price has no monthly figure', async () => {
    getTreeBackfillEstimate.mockResolvedValue({ ...FRESH, costs: [] });
    unmount = await mount(host);
    await settle();
    expect(tiles(host).every((t) => t.includes('No estimate for this model'))).toBe(true);
    expect(host.textContent).not.toContain('a turn.');
  });
});
