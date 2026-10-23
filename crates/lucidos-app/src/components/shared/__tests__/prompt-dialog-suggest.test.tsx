// @vitest-environment jsdom
/**
 * A prompt opened with `suggest` draws a Suggest link beside its message. A tap
 * spins the link and locks the field and OK until the suggestion lands, then
 * fills the field with it, selected. The Rename dialog is the caller.
 */
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

import { PromptDialog } from '../PromptDialog';
import { promptState, showPrompt } from '../../../store/store';
import { _resetOverlayStackForTesting } from '../../../store/overlayStack';

function settled(): Promise<void> {
  return act(() => new Promise<void>((resolve) => setTimeout(resolve, 0)));
}

/** A suggestion the test answers by hand, so the in-flight state is visible. */
function deferredSuggestion() {
  let answer: (value: string | null) => void = () => {};
  const run = () => new Promise<string | null>((resolve) => { answer = resolve; });
  return { run, answer: (value: string | null) => answer(value) };
}

/** Inside the live dialog, never the inert copy a closing one leaves behind. */
const live = <T extends Element>(selector: string) =>
  document.querySelector<T>(`.confirm-dialog:not([inert], [inert] *) ${selector}`);
const field = () => live<HTMLInputElement>('.prompt-input')!;
const link = () => live<HTMLButtonElement>('[data-role="prompt-suggest"]');
const ok = () => live<HTMLButtonElement>('[data-role="prompt-ok"]')!;

describe('the prompt dialog\'s Suggest link', () => {
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

  it('is absent from a prompt opened without one', async () => {
    void showPrompt('Name it.', { defaultValue: 'Old' });
    await settled();
    expect(link()).toBeNull();
  });

  it('spins and locks the field and OK while the suggestion loads, then fills it', async () => {
    const suggestion = deferredSuggestion();
    void showPrompt('Name it.', { defaultValue: 'Old', suggest: suggestion.run });
    await settled();
    expect(link()!.textContent).toBe('Suggest');

    act(() => { link()!.click(); });
    expect(link()!.querySelector('.mini-spinner')).not.toBeNull();
    expect(field().readOnly).toBe(true);
    expect(ok().disabled).toBe(true);

    suggestion.answer('A better name');
    await settled();
    expect(field().value).toBe('A better name');
    expect(field().readOnly).toBe(false);
    expect(ok().disabled).toBe(false);
    expect(link()!.querySelector('.mini-spinner')).toBeNull();
  });

  it('leaves the field as it was when there is nothing to fill', async () => {
    const suggestion = deferredSuggestion();
    void showPrompt('Name it.', { defaultValue: 'Old', suggest: suggestion.run });
    await settled();
    act(() => { link()!.click(); });
    suggestion.answer(null);
    await settled();
    expect(field().value).toBe('Old');
    expect(field().readOnly).toBe(false);
  });

  it('submits the suggestion once the user confirms it', async () => {
    const suggestion = deferredSuggestion();
    const answer = showPrompt('Name it.', { defaultValue: 'Old', suggest: suggestion.run });
    await settled();
    act(() => { link()!.click(); });
    suggestion.answer('A better name');
    await settled();
    act(() => { ok().click(); });
    expect(await answer).toBe('A better name');
  });

  it('drops a suggestion that lands after the prompt was replaced', async () => {
    const suggestion = deferredSuggestion();
    void showPrompt('Name it.', { defaultValue: 'Old', suggest: suggestion.run });
    await settled();
    act(() => { link()!.click(); });
    void showPrompt('Something else?', { defaultValue: 'Fresh' });
    await settled();
    suggestion.answer('A better name');
    await settled();
    expect(field().value).toBe('Fresh');
    expect(field().readOnly).toBe(false);
  });
});
