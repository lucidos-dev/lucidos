// @vitest-environment jsdom
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

vi.mock('../../../store/actions/credentials', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/actions/credentials')>()),
  submitNewCredential: vi.fn(async () => true),
  deleteCredential: vi.fn(async () => {}),
}));

vi.mock('../../../store/actions/preferences', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/actions/preferences')>()),
  savePreference: vi.fn(async () => {}),
  setSystemOneEnabled: vi.fn(async () => {}),
}));

import { CloudflareWorkersAiSettings } from '../CloudflareWorkersAiSettings';
import { CustomSystemOneSettings, customEndpointUrlValid } from '../CustomSystemOneSettings';
import { submitNewCredential } from '../../../store/actions/credentials';
import { savePreference } from '../../../store/actions/preferences';
import { credentials, preferences } from '../../../store/store';
import type { CredentialInfo } from '../../../store/types';

describe('System One provider rows', () => {
  let host: HTMLElement;

  const toggle = () => host.querySelector<HTMLInputElement>('.toggle-switch input');
  const inputs = () => [...host.querySelectorAll<HTMLInputElement>('input.settings-text-input')];
  const button = (text: string) =>
    [...host.querySelectorAll<HTMLButtonElement>('button')].find((b) => b.textContent === text);

  async function reveal(): Promise<void> {
    const input = toggle();
    if (!input) throw new Error('the enable switch is not rendered');
    input.checked = true;
    input.dispatchEvent(new Event('change', { bubbles: true }));
    await new Promise((resolve) => setTimeout(resolve, 0));
  }

  function type(input: HTMLInputElement, value: string): void {
    act(() => {
      input.value = value;
      input.dispatchEvent(new Event('input', { bubbles: true }));
    });
  }

  beforeEach(() => {
    host = document.createElement('div');
    document.body.appendChild(host);
    preferences.value = { status: 'loaded', data: {} };
    credentials.value = { status: 'loaded', data: [] };
    vi.mocked(submitNewCredential).mockClear();
    vi.mocked(savePreference).mockClear();
  });

  afterEach(() => {
    render(null, host);
    host.remove();
    preferences.value = { status: 'not-loaded' };
    credentials.value = { status: 'not-loaded' };
  });

  it('saves a Cloudflare token scoped to its account', async () => {
    act(() => { render(<CloudflareWorkersAiSettings />, host); });
    await reveal();
    const [account, token] = inputs();
    type(account, 'abc123');
    type(token, 'cf-secret');
    act(() => { button('Save')!.click(); });
    expect(submitNewCredential).toHaveBeenCalledWith(
      'cloudflare-workers-ai',
      ['https://api.cloudflare.com/client/v4/accounts/abc123/ai'],
      'api_key',
      'cf-secret',
    );
  });

  it('refuses to save a Cloudflare token without a valid account', async () => {
    act(() => { render(<CloudflareWorkersAiSettings />, host); });
    await reveal();
    const [account, token] = inputs();
    type(account, 'abc/../x');
    type(token, 'cf-secret');
    expect(button('Save')!.disabled).toBe(true);
  });

  it('shows the stored account', () => {
    credentials.value = {
      status: 'loaded',
      data: [{
        id: 'c1',
        service_name: 'cloudflare-workers-ai',
        base_urls: ['https://api.cloudflare.com/client/v4/accounts/abc123/ai'],
        auth_type: 'api_key',
        auth_header: 'Authorization',
        created_at: '2026-10-04T00:00:00Z',
      } as CredentialInfo],
    };
    act(() => { render(<CloudflareWorkersAiSettings />, host); });
    expect(host.textContent).toContain('account abc123');
  });

  it('saves the custom endpoint URL and model', async () => {
    act(() => { render(<CustomSystemOneSettings />, host); });
    await reveal();
    const [url, model] = inputs();
    type(url, ' http://localhost:8000/v1/systemone ');
    type(model, 'kev');
    act(() => { button('Save endpoint')!.click(); });
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(savePreference).toHaveBeenCalledWith(
      'system_one_custom_url',
      'http://localhost:8000/v1/systemone',
    );
    expect(savePreference).toHaveBeenCalledWith('system_one_custom_model', 'kev');
  });

  it('accepts only an http(s) URL for the custom endpoint', () => {
    expect(customEndpointUrlValid('https://decider.example/v1/systemone')).toBe(true);
    expect(customEndpointUrlValid('http://localhost:8000/systemone')).toBe(true);
    expect(customEndpointUrlValid('ftp://localhost/x')).toBe(false);
    expect(customEndpointUrlValid('localhost:8000')).toBe(false);
    expect(customEndpointUrlValid('')).toBe(false);
  });
});
