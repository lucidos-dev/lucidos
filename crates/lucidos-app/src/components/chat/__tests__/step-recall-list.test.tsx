// @vitest-environment jsdom
/** A recall step's detail lists the memories the model saw, each a link to its
 *  source, then the queries that found them. A tap closes the detail and
 *  navigates, so no row is a dead tap. */
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { render } from 'preact';

const showEventWhereItLives = vi.hoisted(() => vi.fn(async () => {}));
const openFilePreview = vi.hoisted(() => vi.fn());
vi.mock('../../../store/actions/event-navigation', () => ({ showEventWhereItLives }));
vi.mock('../../../store/actions/artifacts', () => ({ openFilePreview }));

import { StepDetailModal } from '../StepDetailModal';
import { stepDetailModal } from '../../../store/store';
import type { RecalledMemory } from '../../../store/types';

const memories: RecalledMemory[] = [
  {
    id: 'm1', topic: 'family', summary: 'Birthday is January 1',
    src_created_at: '2026-01-02T03:04:05Z', source: { type: 'event', id: 'e1' },
  },
  {
    id: 'm2', topic: 'notes', summary: 'Party plan',
    src_created_at: '2026-01-03T03:04:05Z', source: { type: 'artifact', path: 'party.md', commit: 'abc' },
  },
];

let host: HTMLDivElement;

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
  showEventWhereItLives.mockClear();
  openFilePreview.mockClear();
});

afterEach(() => {
  render(null, host);
  host.remove();
  stepDetailModal.value = null;
});

function openRecall(recall: { memories: RecalledMemory[]; queries: string[] }): HTMLElement {
  stepDetailModal.value = {
    type: 'step', description: 'Recalled 2 memories', outcome: 'success', recall,
  };
  render(<StepDetailModal />, host);
  return document.querySelector<HTMLElement>('[data-role="step-detail-modal"]')!;
}

function rows(modal: HTMLElement): HTMLButtonElement[] {
  return [...modal.querySelectorAll<HTMLButtonElement>('[data-role="recalled-memories"] button')];
}

it('lists each memory with its topic and summary, in order', () => {
  const modal = openRecall({ memories, queries: ['birthday'] });
  const listed = rows(modal);
  expect(listed.map((b) => b.querySelector('.step-recall-summary')?.textContent))
    .toEqual(['Birthday is January 1', 'Party plan']);
  expect(listed[0].querySelector('.step-recall-meta')?.textContent).toContain('family');
});

it('an event source closes the detail and opens the event where it lives', () => {
  rows(openRecall({ memories, queries: [] }))[0].click();
  expect(stepDetailModal.value).toBeNull();
  expect(showEventWhereItLives).toHaveBeenCalledWith('e1');
  expect(openFilePreview).not.toHaveBeenCalled();
});

it('an artifact source closes the detail and opens the file', () => {
  rows(openRecall({ memories, queries: [] }))[1].click();
  expect(stepDetailModal.value).toBeNull();
  expect(openFilePreview).toHaveBeenCalledWith('artifacts/party.md');
  expect(showEventWhereItLives).not.toHaveBeenCalled();
});

it('shows the queries as a list, not one comma-joined line', () => {
  const modal = openRecall({ memories: [], queries: ['e2e runs', 'nightly status'] });
  expect(rows(modal)).toHaveLength(0);
  const items = [...modal.querySelectorAll('[data-role="recall-queries"] li')].map((li) => li.textContent);
  expect(items).toEqual(['e2e runs', 'nightly status']);
  expect(modal.textContent).not.toContain('e2e runs, nightly status');
});
