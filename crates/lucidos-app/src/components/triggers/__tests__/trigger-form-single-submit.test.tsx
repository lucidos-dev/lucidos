// @vitest-environment jsdom
/**
 * Every create POST mints a new trigger. A second Save while the first is in
 * flight must not send another one, or two live copies fire on schedule.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';

let resolveCreate: (value: { success: boolean }) => void = () => {};
const createTrigger = vi.fn(
  () => new Promise<{ success: boolean }>((resolve) => { resolveCreate = resolve; }),
);

vi.mock('../../../api/client', async () => {
  const actual = await vi.importActual<Record<string, unknown>>('../../../api/client');
  return {
    ...actual,
    fetchEventTypes: vi.fn(async () => []),
    createTrigger: (...args: unknown[]) => createTrigger(...(args as [])),
    listTriggers: vi.fn(async () => []),
    listHistoricalTriggers: vi.fn(async () => []),
  };
});

import { TriggerDetails } from '../TriggerDetails';
import { triggerGroups, panelOverlay, chatModels, triggers } from '../../../store/store';

import { stubIntentFieldObservers } from './intentFieldStubs';

stubIntentFieldObservers();

async function waitFor(done: () => boolean, budgetMs = 1000): Promise<void> {
  const start = Date.now();
  while (!done()) {
    if (Date.now() - start > budgetMs) throw new Error('condition never held');
    await new Promise((r) => setTimeout(r, 5));
  }
}

function type(el: HTMLInputElement | HTMLTextAreaElement, value: string): void {
  el.value = value;
  el.dispatchEvent(new Event('input', { bubbles: true }));
}

describe('the new-trigger form', () => {
  let host: HTMLElement;

  const field = <T extends Element>(selector: string): T => {
    const el = host.querySelector<T>(selector);
    if (!el) throw new Error(`${selector} is not rendered`);
    return el;
  };

  beforeEach(() => {
    document.body.innerHTML = '';
    host = document.createElement('div');
    document.body.appendChild(host);
    chatModels.value = { status: 'loaded', data: [] };
    triggerGroups.value = { status: 'loaded', data: [] };
    panelOverlay.value = { type: 'form', form: { type: 'trigger' } };
    createTrigger.mockClear();
  });

  afterEach(() => {
    render(null, host);
    panelOverlay.value = null;
    triggers.value = { status: 'not-loaded' };
    triggerGroups.value = { status: 'not-loaded' };
    chatModels.value = { status: 'not-loaded' };
  });

  it('sends one create when Save is pressed twice before the first returns', async () => {
    render(<TriggerDetails />, host);

    type(field<HTMLInputElement>('input[placeholder="e.g. Morning Brief"]'), 'Morning brief');
    type(field<HTMLInputElement>('input[placeholder="0 0 8 * * *"]'), '0 0 8 * * *');
    type(field<HTMLTextAreaElement>('textarea.prompt-textarea'), 'summarize my day');
    await waitFor(() => field<HTMLTextAreaElement>('textarea.prompt-textarea').value === 'summarize my day');

    const form = field<HTMLFormElement>('form');
    form.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true }));
    form.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true }));
    await waitFor(() => createTrigger.mock.calls.length > 0);
    await new Promise((r) => setTimeout(r, 20));

    expect(createTrigger).toHaveBeenCalledTimes(1);
    expect(field<HTMLButtonElement>('button.btn-save').disabled).toBe(true);

    resolveCreate({ success: true });
  });
});
