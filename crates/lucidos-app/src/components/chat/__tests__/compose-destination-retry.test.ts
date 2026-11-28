
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
import { describe, expect, it, vi } from 'vitest';
import { retryFailedDestinationLists } from '../ComposeDestinationRow';
import { repositories, appsList } from '../../../store/store';

const composeRowSource = readFileSync(new URL('../ComposeDestinationRow.tsx', import.meta.url), 'utf-8');

function retryDeps() {
  return { loadRepos: vi.fn(), loadAppsList: vi.fn() };
}

describe('compose destination picker retries a failed list on open', () => {
  it('refetches repositories when their load failed', () => {
    repositories.value = { status: 'failed', error: 'request timed out' };
    appsList.value = { status: 'loaded', data: [] };
    const deps = retryDeps();

    retryFailedDestinationLists(deps);

    expect(deps.loadRepos).toHaveBeenCalledOnce();
    expect(deps.loadAppsList).not.toHaveBeenCalled();
  });

  it('refetches apps when their load failed', () => {
    repositories.value = { status: 'loaded', data: [] };
    appsList.value = { status: 'failed', error: 'request cancelled' };
    const deps = retryDeps();

    retryFailedDestinationLists(deps);

    expect(deps.loadAppsList).toHaveBeenCalledOnce();
    expect(deps.loadRepos).not.toHaveBeenCalled();
  });

  it('leaves loaded / loading / not-loaded lists alone — no refetch on every open', () => {
    for (const state of [
      { status: 'loaded' as const, data: [] },
      { status: 'loading' as const },
      { status: 'not-loaded' as const },
    ]) {
      repositories.value = state;
      appsList.value = state;
      const deps = retryDeps();

      retryFailedDestinationLists(deps);

      expect(deps.loadRepos).not.toHaveBeenCalled();
      expect(deps.loadAppsList).not.toHaveBeenCalled();
    }
  });

  it('wires the retry to the destination picker opening', () => {
    const composeDropdowns = composeRowSource.match(/<Dropdown[\s\S]*?\/>/g) ?? [];
    expect(composeDropdowns[0]).toMatch(/class="compose-destination-picker"[\s\S]*onOpen=\{retryFailedDestinationLists\}/);
  });
});
