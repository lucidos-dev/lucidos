// @vitest-environment jsdom
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { render } from 'preact';

import { PromptDialog } from '../PromptDialog';
import { promptState, showPrompt } from '../../../store/store';
import { _resetOverlayStackForTesting } from '../../../store/overlayStack';

const button = (role: string) => document.querySelector<HTMLButtonElement>(`.confirm-dialog [data-role="${role}"]`);

async function drawn(): Promise<void> {
  for (let i = 0; i < 20 && !button('prompt-ok'); i++) await Promise.resolve();
  if (!button('prompt-ok')) throw new Error('the prompt never drew');
}

describe('pressing a prompt dialog button', () => {
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

  it.each(['prompt-ok', 'prompt-cancel', 'prompt-suggest'])('keeps focus in the field for %s', async (role) => {
    void showPrompt('Name it.', { suggest: async () => null });
    await drawn();
    const press = new MouseEvent('mousedown', { bubbles: true, cancelable: true });
    button(role)!.dispatchEvent(press);
    expect(press.defaultPrevented).toBe(true);
  });
});
