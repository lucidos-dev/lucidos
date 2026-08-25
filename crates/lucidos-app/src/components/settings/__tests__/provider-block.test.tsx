// @vitest-environment jsdom
/**
 * The provider block on Settings → Models → Providers.
 *
 * Three things the page depends on and the pure state machine cannot show. The
 * header row survives every state, since a deep link has to land on it. The
 * config rows come and go with the switch. And a press writes the preference
 * only where there is something to switch.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';

vi.mock('../../../store/actions/preferences', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/actions/preferences')>()),
  setProviderEnabled: vi.fn(async () => {}),
}));

import { ProviderBlock } from '../ProviderBlock';
import { setProviderEnabled } from '../../../store/actions/preferences';
import { configuredProviders, preferences } from '../../../store/store';

/** The block under test, with one recognisable config row inside it. */
function block(hasStoredConfig = false) {
  return (
    <ProviderBlock
      id="openai"
      label="OpenAI (direct)"
      anchor="models:openai"
      explainer={<p>What this provider is.</p>}
      hasStoredConfig={hasStoredConfig}
    >
      <div class="settings-row" data-role="secret-row">
        <span class="settings-row-label">Secret</span>
      </div>
    </ProviderBlock>
  );
}

describe('ProviderBlock', () => {
  let host: HTMLElement;

  const header = () => host.querySelector('[data-search-anchor="models:openai"]');
  const configRow = () => host.querySelector('[data-role="secret-row"]');
  const toggle = () => {
    const el = host.querySelector<HTMLInputElement>('.toggle-switch input');
    if (!el) throw new Error('the enable switch is not rendered');
    return el;
  };
  /** Flip the switch and let Preact's queued rerender land. */
  const press = async (next: boolean): Promise<void> => {
    const input = toggle();
    input.checked = next;
    input.dispatchEvent(new Event('change', { bubbles: true }));
    await new Promise((resolve) => setTimeout(resolve, 0));
  };

  beforeEach(() => {
    document.body.innerHTML = '';
    host = document.createElement('div');
    document.body.appendChild(host);
    preferences.value = { status: 'loaded', data: {} };
    configuredProviders.value = [];
    vi.mocked(setProviderEnabled).mockClear();
  });

  afterEach(() => {
    render(null, host);
    document.body.innerHTML = '';
    preferences.value = { status: 'not-loaded' };
    configuredProviders.value = null;
  });

  it('shows the config of a provider the engine is serving', () => {
    configuredProviders.value = ['openai'];
    render(block(), host);
    expect(header()).not.toBeNull();
    expect(configRow()).not.toBeNull();
    expect(toggle().checked).toBe(true);
  });

  it('keeps the header but hides the config while the provider is off', () => {
    render(block(), host);
    // The header has to stay: it carries the deep-link anchor, the explainer
    // and the switch itself, which is the only way back on.
    expect(header()).not.toBeNull();
    expect(configRow()).toBeNull();
    expect(toggle().checked).toBe(false);
  });

  it('reveals the fields without writing, for a provider never set up', async () => {
    render(block(), host);
    await press(true);
    expect(configRow()).not.toBeNull();
    // A stored `false` here would veto a key added later by env var, over a
    // press the user meant as "show me the fields".
    expect(setProviderEnabled).not.toHaveBeenCalled();
  });

  it('switches a running provider off, and says the key was kept', async () => {
    configuredProviders.value = ['openai'];
    render(block(), host);
    await press(false);
    expect(setProviderEnabled).toHaveBeenCalledWith('openai', false);
    expect(configRow()).toBeNull();

    // What the header says once the engine has dropped it. Without this the
    // parked key is invisible and the row reads as never set up.
    preferences.value = { status: 'loaded', data: { provider_enabled_openai: 'false' } };
    configuredProviders.value = [];
    render(block(true), host);
    expect(header()!.textContent).toContain('switched off, key kept');
  });

  it('draws no switch position before the provider list has answered', () => {
    // An env-configured provider stores nothing here, so a guess in this window
    // renders a running provider as never set up. A press against that guess
    // resolves to a local expand and writes nothing, losing the user's intent.
    configuredProviders.value = null;
    render(block(), host);
    expect(header()).not.toBeNull();
    expect(configRow()).toBeNull();
    expect(host.querySelector('.toggle-switch-loading')).not.toBeNull();
    // The placeholder is a span, so there is no input to press at all.
    expect(host.querySelector('.toggle-switch input')).toBeNull();
  });

  it('promises no kept key where this page stored none', () => {
    // Vertex has no credential to keep, and an env-configured provider's key
    // was never ours. Saying "key kept" there would name a key nobody stored.
    preferences.value = { status: 'loaded', data: { provider_enabled_openai: 'false' } };
    render(block(false), host);
    expect(header()!.textContent).toContain('switched off');
    expect(header()!.textContent).not.toContain('key kept');
  });

  it('gives the switch back to the engine when a press is never agreed with', async () => {
    // The write can be refused (`savePreference` toasts rather than rejecting,
    // so there is no promise to catch), or accepted and not applied. Held for
    // good, the switch would sit in a position nothing backs.
    //
    // Only `setTimeout` is faked. Preact flushes renders on a microtask and
    // effects on a frame. Faking those would stop the component rendering at
    // all, and `flush` below rides both real ones.
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
    const flush = async (): Promise<void> => {
      await new Promise((resolve) => { requestAnimationFrame(() => resolve(null)); });
      await Promise.resolve();
    };
    try {
      configuredProviders.value = ['openai'];
      render(block(true), host);
      toggle().checked = false;
      toggle().dispatchEvent(new Event('change', { bubbles: true }));
      await flush();
      expect(configRow()).toBeNull();

      // Nothing moved: the provider is still installed and the press is still
      // held. The fuse hands the position back to what is installed.
      vi.advanceTimersByTime(9000);
      await flush();
      expect(configRow()).not.toBeNull();
      expect(toggle().checked).toBe(true);
    } finally {
      vi.useRealTimers();
    }
  });

  it('switches a parked provider back on', async () => {
    preferences.value = { status: 'loaded', data: { provider_enabled_openai: 'false' } };
    render(block(), host);
    await press(true);
    expect(setProviderEnabled).toHaveBeenCalledWith('openai', true);
  });
});
