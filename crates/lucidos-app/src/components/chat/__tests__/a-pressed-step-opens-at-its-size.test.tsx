// @vitest-environment jsdom
/** A step from a reloaded thread has its command and result stripped. A modal
 *  that fetched them on open painted a bare head, then grew mid-fade, which
 *  read as a flicker. Pressing the row now starts the fetch, and a modal opened
 *  on a fetched step draws its final body on the first frame. */
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

const api = vi.hoisted(() => ({
  fetchToolArgs: vi.fn(),
  fetchToolResult: vi.fn(),
}));

vi.mock('../../../api/threads', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../api/threads')>()),
  fetchToolArgs: api.fetchToolArgs,
  fetchToolResult: api.fetchToolResult,
}));

import { StepDetailModal } from '../StepDetailModal';
import { InlineStep } from '../chat-exchange-parts';
import { stepDetailModal } from '../../../store/store';
import { clearStepDetailCache } from '../../../store/stepDetailCache';
import type { ResponseEvent } from '../../../store/types';

const step: Extract<ResponseEvent, { type: 'step' }> = {
  type: 'step',
  description: 'Running: make',
  outcome: 'success',
  args_stripped: true,
  call_event_id: 'call-1',
  tool_name: 'Bash',
  tool_channel: 'coding_agent',
  result_stripped: true,
  result_event_id: 'result-1',
};

let host: HTMLDivElement;

/** Lets a mocked fetch settle, rejection chains included. */
const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

beforeEach(() => {
  api.fetchToolArgs.mockResolvedValue({ args: { command: 'make test' } });
  api.fetchToolResult.mockResolvedValue({ result: 'all green' });
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
  stepDetailModal.value = null;
  clearStepDetailCache();
  vi.clearAllMocks();
});

async function pressRow(): Promise<HTMLButtonElement> {
  act(() => { render(<InlineStep event={step} />, host); });
  const main = host.querySelector<HTMLButtonElement>('[data-role="step-main"]')!;
  await act(async () => {
    main.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true }));
    await settle();
  });
  return main;
}

it('starts both fetches on press, before the click', async () => {
  await pressRow();
  expect(api.fetchToolArgs).toHaveBeenCalledWith('call-1');
  expect(api.fetchToolResult).toHaveBeenCalledWith('result-1');
  expect(stepDetailModal.value).toBeNull();
});

it('draws the fetched command and result on the first frame', async () => {
  const main = await pressRow();
  act(() => { main.click(); });

  const modalHost = document.createElement('div');
  document.body.appendChild(modalHost);
  // Rendered once, with no await: this is the frame the modal opens on.
  act(() => { render(<StepDetailModal />, modalHost); });
  const modal = document.querySelector<HTMLElement>('[data-role="step-detail-modal"]')!;
  expect(modal.querySelector('.step-detail-full')?.textContent).toBe('make test');
  expect(modal.querySelector('.step-detail-result')?.textContent).toBe('all green');
  render(null, modalHost);
  modalHost.remove();
});

it('fetches a step once, however often it is pressed and opened', async () => {
  const main = await pressRow();
  await act(async () => {
    main.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true }));
    await settle();
  });
  stepDetailModal.value = step;
  act(() => { render(<StepDetailModal />, host); });
  expect(api.fetchToolArgs).toHaveBeenCalledTimes(1);
  expect(api.fetchToolResult).toHaveBeenCalledTimes(1);
});

it('retries a failed fetch when the step is opened again', async () => {
  api.fetchToolResult.mockRejectedValueOnce(new Error('offline'));
  stepDetailModal.value = step;
  await act(async () => {
    render(<StepDetailModal />, host);
    await settle();
  });
  expect(document.querySelector('[data-role="result-error"]')?.textContent).toContain('offline');

  act(() => { render(null, host); });
  await act(async () => {
    render(<StepDetailModal />, host);
    await settle();
  });
  expect(api.fetchToolResult).toHaveBeenCalledTimes(2);
  expect(document.querySelector('.step-detail-result')?.textContent).toBe('all green');
});
