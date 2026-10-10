// @vitest-environment jsdom
/** A step opened from a reloaded thread fetches its command and result. Past
 *  the delay gate each draws a skeleton in the shape of its own block, never a
 *  line of "Loading…" text. */
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

const pendingResult = vi.hoisted(() => ({ value: new Promise<{ result: string | null }>(() => {}) }));

vi.mock('../../../api/threads', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../api/threads')>()),
  fetchToolArgs: () => new Promise(() => {}),
  fetchToolResult: () => pendingResult.value,
}));

import { StepDetailModal } from '../StepDetailModal';
import { stepDetailModal } from '../../../store/store';
import { clearStepDetailCache } from '../../../store/stepDetailCache';
import { SPINNER_DELAY_MS } from '../../../hooks/useDelayedLoading';

let host: HTMLDivElement;

beforeEach(() => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
  host = document.createElement('div');
  document.body.appendChild(host);
  stepDetailModal.value = {
    type: 'step',
    description: 'Running: make test',
    outcome: 'success',
    args_stripped: true,
    call_event_id: 'call-1',
    tool_name: 'Bash',
    result_stripped: true,
    result_event_id: 'result-1',
  };
});

afterEach(() => {
  render(null, host);
  host.remove();
  stepDetailModal.value = null;
  clearStepDetailCache();
  vi.useRealTimers();
  pendingResult.value = new Promise(() => {});
});

function openModal(): HTMLElement {
  act(() => { render(<StepDetailModal />, host); });
  return document.querySelector<HTMLElement>('[data-role="step-detail-modal"]')!;
}

it('draws nothing in the body before the delay gate opens', () => {
  const modal = openModal();
  expect(modal.querySelector('.sk-bar')).toBeNull();
  expect(modal.querySelector('.surface-body')?.textContent).toBe('');
});

it('draws the command and result blocks as skeletons once the gate opens', () => {
  const modal = openModal();
  act(() => { vi.advanceTimersByTime(SPINNER_DELAY_MS); });
  expect(modal.querySelector('.step-detail-full .sk-bar')).not.toBeNull();
  expect(modal.querySelector('.step-detail-result .sk-bar')).not.toBeNull();
  expect(modal.textContent).not.toContain('Loading');
});

it('leaves no wrapper behind when the fetched result is empty', async () => {
  pendingResult.value = Promise.resolve({ result: null });
  const modal = openModal();
  await act(async () => { await pendingResult.value; });
  expect(modal.querySelector('.step-detail-result')).toBeNull();
  expect(modal.querySelectorAll('.step-detail-fade')).toHaveLength(1);
});
