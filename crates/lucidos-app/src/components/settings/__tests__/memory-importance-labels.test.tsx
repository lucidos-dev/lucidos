// @vitest-environment jsdom
/** The importance bar's tooltips name each band's score range in words,
 *  "Low (0 to 0.3)", never with a dash. */
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { render, type FunctionComponent } from 'preact';
import { act } from 'preact/test-utils';
import type { MemoryStatsResponse } from '../../../api/types';
import { rangeLabel } from '../../../utils/rangeLabel';

const reads = vi.hoisted(() => ({ stats: (_: MemoryStatsResponse) => {} }));

vi.mock('../../shared/Disclosure', () => import('../../shared/__tests__/disclosureStub'));
vi.mock('../../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../api/client')>()),
  getMemoryStats: () => new Promise((r) => { reads.stats = r; }),
  getMemoryEntries: () => new Promise(() => {}),
  getMemorySource: () => new Promise(() => {}),
}));

let host: HTMLDivElement;
let MemoryInspector: FunctionComponent;

beforeEach(async () => {
  vi.resetModules();
  ({ MemoryInspector } = await import('../MemoryInspector'));
  host = document.createElement('div');
  document.body.appendChild(host);
  act(() => { render(<MemoryInspector />, host); });
});

afterEach(() => {
  render(null, host);
  host.remove();
});

it('labels each importance band with a dash-free range', async () => {
  await act(async () => {
    reads.stats({
      total: 10,
      event_count: 10,
      artifact_count: 0,
      importance_distribution: { low: 1, medium: 2, high: 3, critical: 4 },
      top_topics: [],
    });
  });
  const labels = [...host.querySelectorAll('.memory-importance-segment')]
    .map((segment) => segment.getAttribute('data-tooltip'));
  expect(labels).toEqual([
    `Low (${rangeLabel('0', '0.3')}): 1`,
    `Medium (${rangeLabel('0.3', '0.6')}): 2`,
    `High (${rangeLabel('0.6', '0.8')}): 3`,
    `Critical (${rangeLabel('0.8', '1.0')}): 4`,
  ]);
});
