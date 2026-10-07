// @vitest-environment jsdom
/**
 * Choosing Tree spends money, so the first time the Tree button only opens a
 * confirm panel with the estimate. `memory_module` is written by its Start
 * button alone, and Start waits for the estimate. Cancel writes nothing. Once
 * a backfill has started, Tree is written at once.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import type { BackgroundModels, TreeBackfillEstimate } from '../../../api/types';

const { getTreeBackfillEstimate, getTreeBackfill, getBackgroundModels, setMemoryModule } = vi.hoisted(() => ({
  getTreeBackfillEstimate: vi.fn<() => Promise<TreeBackfillEstimate>>(),
  getTreeBackfill: vi.fn(),
  getBackgroundModels: vi.fn<() => Promise<BackgroundModels>>(),
  setMemoryModule: vi.fn<(m: string) => Promise<void>>(),
}));

vi.mock('../../../api/client', async () => {
  const actual = await vi.importActual<Record<string, unknown>>('../../../api/client');
  return { ...actual, getTreeBackfillEstimate, getTreeBackfill, getBackgroundModels };
});

vi.mock('../../../store/actions/preferences', async () => {
  const actual = await vi.importActual<Record<string, unknown>>('../../../store/actions/preferences');
  return { ...actual, setMemoryModule };
});

import { stubIntentFieldObservers } from '../../triggers/__tests__/intentFieldStubs';

stubIntentFieldObservers();

const ESTIMATE: TreeBackfillEstimate = {
  calls: { low: 210_000, high: 260_000 },
  usable_secs: { low: 20 * 60, high: 80 * 60 },
  complete_secs: { low: 6 * 3600, high: 19 * 3600 },
  daily_calls: { low: 700, high: 900 },
  costs: [{
    model: 'gpt-6.1-sol',
    usable_secs: { low: 30 * 60, high: 30 * 60 },
    complete_secs: { low: 2 * 3600, high: 3 * 3600 },
    by_effort: [{
      effort: 'low',
      backfill_usd: { low: 1100, high: 2300 },
      backfill_usd_central: 1700,
      daily_usd: { low: 12, high: 30 },
      daily_usd_central: 20,
    }],
  }],
};

async function mount(host: HTMLElement): Promise<() => void> {
  const { h, render } = await import('preact');
  const store = await import('../../../store/store');
  const { MemoryModuleSection } = await import('../MemoryModuleSection');
  store.preferences.value = { status: 'loaded', data: { memory_module: 'classic' } };
  render(h(MemoryModuleSection, {}), host);
  return () => render(null, host);
}

function button(host: HTMLElement, name: string): HTMLButtonElement {
  const found = [...host.querySelectorAll('button')].find((b) => b.textContent?.trim() === name);
  if (!found) throw new Error(`no "${name}" button`);
  return found;
}

/** Preact runs effects after paint, which jsdom only reaches through the
 *  100 ms fallback timer, so a settle waits past it. */
async function settle(): Promise<void> {
  await new Promise((r) => setTimeout(r, 150));
}

describe('choosing Tree', () => {
  let host: HTMLElement;
  let unmount: () => void = () => {};

  beforeEach(() => {
    host = document.createElement('div');
    document.body.appendChild(host);
    getTreeBackfillEstimate.mockReset();
    getTreeBackfill.mockReset().mockResolvedValue({ state: 'off', started: false });
    // The engine's provider-aware default, which the panel prices while the
    // compactor preferences are unset.
    getBackgroundModels.mockReset().mockResolvedValue({
      model_summary_compaction: {
        model: 'gpt-6.1-sol',
        effort: 'low',
        source: 'default',
        reachable: true,
        needs_vision: false,
        vision: true,
        recommended: ['gpt-6.1-sol', 'gemini-3.8-flash', 'claude-sonnet-5-5'],
      },
    });
    setMemoryModule.mockReset().mockResolvedValue(undefined);
  });

  afterEach(() => {
    unmount();
    host.remove();
  });

  it('opens the estimate, and writes nothing until Start', async () => {
    let resolve: (e: TreeBackfillEstimate) => void = () => {};
    getTreeBackfillEstimate.mockReturnValue(new Promise((r) => { resolve = r; }));
    unmount = await mount(host);

    button(host, 'Tree').click();
    await settle();
    expect(host.querySelector('[data-role="tree-confirm"]')).not.toBeNull();
    expect(setMemoryModule).not.toHaveBeenCalled();
    expect(button(host, 'Start Tree').disabled).toBe(true);

    resolve(ESTIMATE);
    await settle();
    const estimate = host.querySelector('[data-role="tree-estimate"]')?.textContent ?? '';
    expect(estimate).toContain('210K–260K');
    expect(estimate).toContain('$1,100–$2,300');
    // The chosen model's own times, from its measured call time.
    expect(estimate).toContain('Usable in30 minutes');
    expect(estimate).toContain('Complete in2–3 hours');
    expect(button(host, 'Start Tree').disabled).toBe(false);
    expect(setMemoryModule).not.toHaveBeenCalled();

    button(host, 'Start Tree').click();
    await settle();
    expect(setMemoryModule).toHaveBeenCalledExactlyOnceWith('tree');
  });

  it('writes nothing on Cancel', async () => {
    getTreeBackfillEstimate.mockResolvedValue(ESTIMATE);
    unmount = await mount(host);

    button(host, 'Tree').click();
    await settle();
    button(host, 'Cancel').click();
    await settle();
    expect(setMemoryModule).not.toHaveBeenCalled();
  });

  it('shows only the error when the estimate fails, and Try again reads it again', async () => {
    getTreeBackfillEstimate
      .mockRejectedValueOnce(new Error('Request timed out after 60000ms'))
      .mockResolvedValueOnce(ESTIMATE);
    unmount = await mount(host);

    button(host, 'Tree').click();
    await settle();
    expect(host.querySelector('[data-role="tree-estimate"]')).toBeNull();
    expect(host.textContent).toContain('Could not estimate the cost: Request timed out after 60000ms');
    expect(button(host, 'Start Tree').disabled).toBe(true);

    button(host, 'Try again').click();
    await settle();
    expect(getTreeBackfillEstimate).toHaveBeenCalledTimes(2);
    expect(host.querySelector('[data-role="tree-estimate"]')?.textContent).toContain('210K–260K');
    expect(host.textContent).not.toContain('Could not estimate the cost');
    expect(button(host, 'Start Tree').disabled).toBe(false);
  });

  /** Every model is offered, but only the measured ones are priced. */
  it('says a model it cannot price has no estimate', async () => {
    getTreeBackfillEstimate.mockResolvedValue({ ...ESTIMATE, costs: [] });
    unmount = await mount(host);

    button(host, 'Tree').click();
    await settle();
    const estimate = host.querySelector('[data-role="tree-estimate"]')?.textContent ?? '';
    expect(estimate).toContain('No estimate for this model');
    // The times fall back to the default seconds per call.
    expect(estimate).toContain('Complete in6–19 hours');
  });

  it('says when it cannot read the compactor model, and still allows Start', async () => {
    getTreeBackfillEstimate.mockResolvedValue(ESTIMATE);
    getBackgroundModels.mockRejectedValue(new Error('engine unreachable'));
    unmount = await mount(host);

    button(host, 'Tree').click();
    await settle();
    expect(host.querySelector('[data-role="tree-confirm"]')?.textContent)
      .toContain('Could not read the default model');
    expect(host.querySelector('[data-role="tree-estimate"]')?.textContent)
      .not.toContain('No estimate for this model');
    expect(button(host, 'Start Tree').disabled).toBe(false);
  });

  it('prices a stored model over the resolved default', async () => {
    getTreeBackfillEstimate.mockResolvedValue({
      ...ESTIMATE,
      costs: [
        ...ESTIMATE.costs,
        {
          model: 'claude-sonnet-5-5',
          usable_secs: ESTIMATE.usable_secs,
          complete_secs: ESTIMATE.complete_secs,
          by_effort: [{
            effort: 'low',
            backfill_usd: { low: 2200, high: 4600 },
            backfill_usd_central: 3400,
            daily_usd: { low: 24, high: 60 },
            daily_usd_central: 40,
          }],
        },
      ],
    });
    const store = await import('../../../store/store');
    unmount = await mount(host);
    store.preferences.value = {
      status: 'loaded',
      data: { memory_module: 'classic', model_summary_compaction: 'claude-sonnet-5-5' },
    };

    button(host, 'Tree').click();
    await settle();
    expect(host.querySelector('[data-role="tree-estimate"]')?.textContent)
      .toContain('$2,200–$4,600');
  });

  it('goes straight back to Tree once a backfill has started', async () => {
    getTreeBackfill.mockResolvedValue({ state: 'off', started: true });
    unmount = await mount(host);
    await settle();

    button(host, 'Tree').click();
    await settle();
    expect(host.querySelector('[data-role="tree-confirm"]')).toBeNull();
    expect(getTreeBackfillEstimate).not.toHaveBeenCalled();
    expect(setMemoryModule).toHaveBeenCalledExactlyOnceWith('tree');
  });

  it('names the compactor model rather than asking for one', async () => {
    const store = await import('../../../store/store');
    unmount = await mount(host);
    store.preferences.value = { status: 'loaded', data: { memory_module: 'tree' } };
    await settle();
    const note = host.querySelector('[data-role="memory-compaction-link"]')?.textContent ?? '';
    expect(note).toContain("gpt-6.1-sol writes Tree's summaries. Change it under");
    expect(note).not.toContain('Pick it');
  });
});
