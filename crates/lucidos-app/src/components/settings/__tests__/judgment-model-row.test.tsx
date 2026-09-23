// @vitest-environment jsdom
/**
 * One classification's model control, with TypeSafe (Jev) among the models.
 *
 * Four things the pure decision functions cannot show. The row waits for BOTH
 * reads before offering Jev. The trigger reads the backend in force. A pick
 * writes the judgment key AND the model pair, in that order. And a pick of Jev
 * leaves the stored model alone.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

vi.mock('../../../store/actions/preferences', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/actions/preferences')>()),
  savePreference: vi.fn(async () => {}),
  saveModelSelection: vi.fn(async () => {}),
}));

import { JudgmentModelRow } from '../JudgmentModelRow';
import { savePreference, saveModelSelection } from '../../../store/actions/preferences';
import { credentials, preferences } from '../../../store/store';
import type { ModelChoice } from '../../../store/modelSelection';
import type { CredentialInfo } from '../../../store/types';

const MODELS: ModelChoice[] = [
  { value: 'claude-haiku-4-5', label: 'Haiku 4.5', reasoningEfforts: ['none', 'low'] },
  { value: 'gemini-3.5-flash', label: 'Gemini 3.5 Flash', reasoningEfforts: [] },
];

function typeSafeCred(): CredentialInfo {
  return {
    id: 'c1',
    service_name: 'typesafe',
    base_urls: ['https://api.typesafe.ai/v1'],
    auth_type: 'api_key',
    auth_header: 'Authorization',
    created_at: '2026-09-19T00:00:00Z',
  } as CredentialInfo;
}

describe('JudgmentModelRow', () => {
  let host: HTMLElement;

  const trigger = () => host.querySelector<HTMLButtonElement>('.dropdown-trigger');
  const triggerLabel = () => host.querySelector('.model-selection-value')?.textContent ?? '';
  const detail = () => host.querySelector('.list-row-details')?.textContent ?? '';
  /** The picker portals to `document.body`, so the options are not under the
   *  host element. Rows are read by `data-value`, which is the model id on the
   *  model step and the encoded pair on the tier step. A row's TEXT carries a
   *  checkmark, a disclosure chevron and a description besides. */
  const options = () => [...document.body.querySelectorAll<HTMLElement>('.control-option')];
  const optionValues = () => options().map((o) => o.dataset.value ?? '');

  /** Every interaction goes through `act`, so the panel the click opens is on
   *  screen before the next line looks for it. */
  function open() {
    act(() => { trigger()!.click(); });
  }

  function pick(value: string) {
    const match = options().find((o) => o.dataset.value === value);
    if (!match) throw new Error(`no option "${value}" in ${optionValues().join(', ')}`);
    act(() => { match.click(); });
  }

  function mount(disabled = false) {
    act(() => {
      render(
        <JudgmentModelRow
          site="command-guard"
          label="Judge model"
          anchor="command-safety:judge-model"
          models={MODELS}
          disabled={disabled}
        />,
        host,
      );
    });
  }

  beforeEach(() => {
    host = document.createElement('div');
    document.body.appendChild(host);
    vi.mocked(savePreference).mockClear();
    vi.mocked(saveModelSelection).mockClear();
    preferences.value = { status: 'loaded', data: {} };
    credentials.value = { status: 'loaded', data: [] };
  });

  afterEach(() => {
    render(null, host);
    host.remove();
    preferences.value = { status: 'not-loaded' };
    credentials.value = { status: 'not-loaded' };
  });

  /** ADR 0220's no-change promise, at the surface the user sees. */
  it('offers only the chat models with no TypeSafe key', () => {
    mount();
    open();
    expect(optionValues()).toEqual(['claude-haiku-4-5', 'gemini-3.5-flash']);
  });

  /** Unloaded credentials read as no key. An ungated rule would drop the row
   *  out of a workspace that HAS one, for as long as the load takes. */
  it('offers no Jev row while the credentials are still loading', () => {
    credentials.value = { status: 'loading' };
    mount();
    open();
    expect(optionValues()).not.toContain('typesafe-jev');
  });

  it('offers the Jev row once a key is stored', () => {
    credentials.value = { status: 'loaded', data: [typeSafeCred()] };
    mount();
    open();
    expect(optionValues()).toEqual(['claude-haiku-4-5', 'gemini-3.5-flash', 'typesafe-jev']);
  });

  it('reads the backend in force on the trigger', () => {
    credentials.value = { status: 'loaded', data: [typeSafeCred()] };
    preferences.value = { status: 'loaded', data: { judgment_command_guard: 'jev' } };
    mount();
    expect(triggerLabel()).toBe('TypeSafe (Jev)');
  });

  it('reads the stored chat pair otherwise', () => {
    preferences.value = {
      status: 'loaded',
      data: { model_command_judge: 'claude-haiku-4-5', reasoning_command_judge: 'low' },
    };
    mount();
    expect(triggerLabel()).toBe('Haiku 4.5 · Low');
  });

  /** One write, and the model key is not it: switching back has to restore the
   *  model the user had. */
  it('writes only the judgment key when Jev is picked', () => {
    credentials.value = { status: 'loaded', data: [typeSafeCred()] };
    mount();
    open();
    pick('typesafe-jev');
    expect(savePreference).toHaveBeenCalledWith('judgment_command_guard', 'jev');
    expect(saveModelSelection).not.toHaveBeenCalled();
  });

  /** Both writes, because the site was on Jev. The model alone would leave the
   *  engine on Jev under a trigger reading a chat model. */
  it('moves the backend back to chat and writes the pair', async () => {
    credentials.value = { status: 'loaded', data: [typeSafeCred()] };
    preferences.value = { status: 'loaded', data: { judgment_command_guard: 'jev' } };
    mount();
    open();
    pick('claude-haiku-4-5');
    pick('claude-haiku-4-5|low');
    expect(savePreference).toHaveBeenCalledWith('judgment_command_guard', 'chat');
    await Promise.resolve();
    expect(saveModelSelection).toHaveBeenCalledWith(
      'model_command_judge',
      'reasoning_command_judge',
      { model: 'claude-haiku-4-5', reasoningEffort: 'low', provider: null },
    );
  });

  /** A tierless model commits on the model step, so there is no second click
   *  and no effort to write. */
  it('commits a tierless model in one step', async () => {
    mount();
    open();
    pick('gemini-3.5-flash');
    expect(savePreference).toHaveBeenCalledWith('judgment_command_guard', 'chat');
    await Promise.resolve();
    expect(saveModelSelection).toHaveBeenCalledWith(
      'model_command_judge',
      'reasoning_command_judge',
      { model: 'gemini-3.5-flash', reasoningEffort: null, provider: null },
    );
  });

  /** `jev_for` returns nothing while the master switch is off, so the row must
   *  say the selection is not the one running. */
  it('says so while TypeSafe itself is switched off', () => {
    credentials.value = { status: 'loaded', data: [typeSafeCred()] };
    preferences.value = {
      status: 'loaded',
      data: { judgment_command_guard: 'jev', provider_enabled_typesafe: 'false' },
    };
    mount();
    expect(triggerLabel()).toBe('TypeSafe (Jev)');
    expect(detail()).toContain('Models → Providers');
  });

  /** Still reachable, because the stored pick is Jev and there has to be a way
   *  off it. */
  it('keeps the Jev row offered while TypeSafe is switched off', () => {
    credentials.value = { status: 'loaded', data: [typeSafeCred()] };
    preferences.value = {
      status: 'loaded',
      data: { judgment_command_guard: 'jev', provider_enabled_typesafe: 'false' },
    };
    mount();
    open();
    expect(optionValues()).toContain('typesafe-jev');
  });

  it('honours a disabled prop from the surrounding feature', () => {
    mount(true);
    expect(trigger()?.disabled).toBe(true);
  });
});
