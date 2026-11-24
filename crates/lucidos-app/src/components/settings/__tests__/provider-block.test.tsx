// @vitest-environment jsdom
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';

vi.mock('../../../store/actions/preferences', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/actions/preferences')>()),
  setProviderEnabled: vi.fn(async () => {}),
}));

import { ProviderBlock } from '../ProviderBlock';
import { setProviderEnabled } from '../../../store/actions/preferences';
import { configuredProviders, preferences } from '../../../store/store';

function block(hasStoredConfig = false) {
  return (
    <ProviderBlock
      id="openai"
      label="OpenAI (direct)"
      anchor="providers:openai"
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

  const header = () => host.querySelector('[data-search-anchor="providers:openai"]');
  const configRow = () => host.querySelector('[data-role="secret-row"]');
  const toggle = () => {
    const el = host.querySelector<HTMLInputElement>('.toggle-switch input');
    if (!el) throw new Error('the enable switch is not rendered');
    return el;
  };
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
    expect(header()).not.toBeNull();
    expect(configRow()).toBeNull();
    expect(toggle().checked).toBe(false);
  });

  it('reveals the fields without writing, for a provider never set up', async () => {
    render(block(), host);
    await press(true);
    expect(configRow()).not.toBeNull();
    expect(setProviderEnabled).not.toHaveBeenCalled();
  });

  it('switches a running provider off, and says the key was kept', async () => {
    configuredProviders.value = ['openai'];
    render(block(), host);
    await press(false);
    expect(setProviderEnabled).toHaveBeenCalledWith('openai', false);
    expect(configRow()).toBeNull();

    preferences.value = { status: 'loaded', data: { provider_enabled_openai: 'false' } };
    configuredProviders.value = [];
    render(block(true), host);
    expect(header()!.textContent).toContain('switched off, key kept');
  });

  it('draws no switch position before the provider list has answered', () => {
    configuredProviders.value = null;
    render(block(), host);
    expect(header()).not.toBeNull();
    expect(configRow()).toBeNull();
    expect(host.querySelector('.toggle-switch-loading')).not.toBeNull();
    expect(host.querySelector('.toggle-switch input')).toBeNull();
  });

  it('promises no kept key where this page stored none', () => {
    preferences.value = { status: 'loaded', data: { provider_enabled_openai: 'false' } };
    render(block(false), host);
    expect(header()!.textContent).toContain('switched off');
    expect(header()!.textContent).not.toContain('key kept');
  });

  it('gives the switch back to the engine when a press is never agreed with', async () => {
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

      vi.advanceTimersByTime(9000);
      await flush();
      expect(configRow()).not.toBeNull();
      expect(toggle().checked).toBe(true);
    } finally {
      vi.useRealTimers();
    }
  });

  it('keeps the fields of a provider never set up open past the fuse', async () => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
    const flush = async (): Promise<void> => {
      await new Promise((resolve) => { requestAnimationFrame(() => resolve(null)); });
      await Promise.resolve();
    };
    try {
      render(block(), host);
      toggle().checked = true;
      toggle().dispatchEvent(new Event('change', { bubbles: true }));
      await flush();
      expect(configRow()).not.toBeNull();

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
