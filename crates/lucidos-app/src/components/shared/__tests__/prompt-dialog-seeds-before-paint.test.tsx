// @vitest-environment jsdom
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

import { PromptDialog } from '../PromptDialog';
import { promptState, showPrompt } from '../../../store/store';
import { _resetOverlayStackForTesting } from '../../../store/overlayStack';

const field = () => document.querySelector<HTMLInputElement>('.confirm-dialog:not([inert]) .prompt-input');

async function drawn(): Promise<HTMLInputElement> {
  for (let i = 0; i < 20 && !field(); i++) await Promise.resolve();
  const el = field();
  if (!el) throw new Error('the prompt never drew');
  return el;
}

describe('the prompt dialog\'s field', () => {
  let host: HTMLDivElement;

  beforeEach(() => {
    _resetOverlayStackForTesting();
    promptState.value = { visible: false, message: '' };
    host = document.createElement('div');
    document.body.appendChild(host);
    render(<PromptDialog />, host);
  });

  afterEach(() => {
    promptState.peek().resolve?.(null);
    render(null, host);
    host.remove();
  });

  it('shows the default value as soon as it draws', async () => {
    void showPrompt('Name it.', { defaultValue: 'Old' });
    expect((await drawn()).value).toBe('Old');
  });

  it('keeps text typed right after it draws, and submits it', async () => {
    const answer = showPrompt('Name it.', { defaultValue: 'Old' });
    (await drawn()).value = 'Typed';
    await act(() => new Promise<void>((resolve) => setTimeout(resolve, 50)));
    act(() => { document.querySelector<HTMLButtonElement>('[data-role="prompt-ok"]')!.click(); });
    expect(await answer).toBe('Typed');
  });
});
