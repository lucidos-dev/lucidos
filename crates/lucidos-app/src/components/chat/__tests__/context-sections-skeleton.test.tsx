// @vitest-environment jsdom
/** A context capture from a reloaded thread fetches its sections. Past the
 *  delay gate the step details draw the meta line and role headers as
 *  skeletons, never a line of "Loading sections…". */
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import type { ContextCapture } from '../../../store/types';

vi.mock('../../../api/threads', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../api/threads')>()),
  fetchContextCapture: () => new Promise(() => {}),
}));

import { ContextCapturePanel } from '../ContextCapturePanel';
import { SPINNER_DELAY_MS } from '../../../hooks/useDelayedLoading';

const snap: ContextCapture = {
  producer: 'main_llm',
  model: 'test-model',
  context_window: 200_000,
  sections: [],
  tools: [],
  estimated_total_tokens: 1_000,
  trimmed: false,
  sections_stripped: true,
  event_id: 'capture-1',
};

let host: HTMLDivElement;

beforeEach(() => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
  host = document.createElement('div');
  document.body.appendChild(host);
  act(() => { render(<ContextCapturePanel snap={snap} />, host); });
});

afterEach(() => {
  render(null, host);
  host.remove();
  vi.useRealTimers();
});

it('draws the budget bar at once and no skeleton before the gate opens', () => {
  expect(host.querySelector('[data-role="budget-bar"]')).not.toBeNull();
  expect(host.querySelector('.sk-bar')).toBeNull();
});

it('draws role headers as the skeleton once the gate opens', () => {
  act(() => { vi.advanceTimersByTime(SPINNER_DELAY_MS); });
  expect(host.querySelectorAll('.loading-fade-skeleton .context-role-header')).toHaveLength(3);
  expect(host.querySelector('.loading-fade-skeleton .context-role-label .sk-bar')).not.toBeNull();
  expect(host.textContent).not.toContain('Loading');
});
