// @vitest-environment jsdom
import { describe, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { ComposeHookArguments, firstMessageArguments } from './ComposeHookArguments';
import { getComposeSelectionOverride, patchComposeSelection, clearComposeSelection } from '../../store/composeSelections';
import { fetchEventHooks, fetchHookChoices } from '../../api/client/eventHooks';
import type { EventHook } from '../../api/client/eventHooks';

vi.mock('../../api/client/eventHooks', () => ({
  fetchEventHooks: vi.fn(),
  fetchHookChoices: vi.fn(),
}));

function hook(slug: string, arm_state: EventHook['arm_state'], event_type = 'MessageReceived'): EventHook {
  return {
    slug,
    status: 'valid',
    event_kind: 'thread',
    arm_state,
    armed: null,
    arm_refusal: null,
    remove_refusal: null,
    content_hash: slug,
    choices_timeout_secs: arm_state === 'armed' && event_type === 'MessageReceived' ? 20 : undefined,
    definition: { name: slug, event_type, script: 'check.py', arguments: { ticket: { label: 'Ticket', choices_script: 'choices.py' } } },
  };
}

describe('composer hook arguments', () => {
  it('offers declared arguments from armed first-message hooks', () => {
    expect(firstMessageArguments([
      hook('check-ticket', 'armed'),
      hook('check-other', 'disarmed'),
      hook('changed', 'changed'),
      hook('apply', 'armed', 'ChangeApplyRequested'),
    ])).toEqual([{ slug: 'check-ticket', hash: 'check-ticket', name: 'ticket', label: 'Ticket', timeoutSecs: 20 }]);
  });

  it('lets the user clear a pick or enter a known value while choices are unavailable', async () => {
    vi.mocked(fetchEventHooks).mockResolvedValue([hook('check-ticket', 'armed')]);
    vi.mocked(fetchHookChoices).mockRejectedValue(new Error('Choices unavailable'));
    patchComposeSelection(null, { hookArgs: { ticket: 'TK-42' } });
    const host = document.createElement('div');
    document.body.append(host);
    try {
      act(() => render(<ComposeHookArguments threadId={null} fading={false} />, host));
      const picker = () => host.querySelector<HTMLButtonElement>('.dropdown-trigger');
      await vi.waitFor(() => expect(picker()?.getAttribute('aria-label')).toBe('Ticket: TK-42'));
      act(() => picker()!.click());
      await vi.waitFor(() => expect(document.body.querySelector('[role="status"]')?.textContent).toContain('Choices unavailable'));
      const option = (label: string) => Array.from(document.body.querySelectorAll<HTMLElement>('.dropdown-option'))
        .find(row => row.textContent === label);
      act(() => option('Clear selection')!.click());
      expect(getComposeSelectionOverride(null).hookArgs).toBeUndefined();
      act(() => picker()!.click());
      const input = document.body.querySelector<HTMLInputElement>('.dropdown-filter')!;
      act(() => {
        input.value = 'TK-43';
        input.dispatchEvent(new Event('input', { bubbles: true }));
      });
      await vi.waitFor(() => expect(option('Use “TK-43”')).toBeDefined());
      act(() => option('Use “TK-43”')!.click());
      expect(getComposeSelectionOverride(null).hookArgs).toEqual({ ticket: 'TK-43' });
    } finally {
      act(() => render(null, host));
      host.remove();
      clearComposeSelection(null);
    }
  });
});
