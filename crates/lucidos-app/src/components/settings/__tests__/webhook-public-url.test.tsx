// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { publicWebhookDeliveryUrl, WebhookPublicUrlField } from '../WebhookPublicUrlField';
import { deletePreference, setPreference } from '../../../api/client';
import { loadPreferences } from '../../../store/actions/preferences';
import { showToast } from '../../../store/store';

vi.mock('../../../api/client', () => ({
  deletePreference: vi.fn(),
  setPreference: vi.fn(),
}));
vi.mock('../../../store/actions/preferences', () => ({ loadPreferences: vi.fn() }));
vi.mock('../../../store/store', () => ({ showToast: vi.fn() }));

const path = '/workspace/hook-id';
const host = document.createElement('div');

beforeEach(() => vi.resetAllMocks());
afterEach(() => render(null, host));

it('appends the hook path to a canonical HTTPS origin and preserves the encoded prefix', () => {
  expect(publicWebhookDeliveryUrl(' HTTPS://Hooks.Example:8443/notify%2Fteam/// ', path))
    .toBe('https://hooks.example:8443/notify%2Fteam/workspace/hook-id');
  expect(publicWebhookDeliveryUrl('https://[2001:db8::1]:443', path))
    .toBe('https://[2001:db8::1]/workspace/hook-id');
  expect(publicWebhookDeliveryUrl('', path)).toBeNull();
});

function mount(stored: string): HTMLInputElement {
  act(() => render(<WebhookPublicUrlField stored={stored} />, host));
  return host.querySelector('input')!;
}

function edit(input: HTMLInputElement, value: string) {
  act(() => {
    input.value = value;
    input.dispatchEvent(new Event('input', { bubbles: true }));
  });
}

async function submit() {
  await act(async () => {
    host.querySelector('form')!.dispatchEvent(new Event('submit', { cancelable: true }));
  });
}

it('saves a URL and refreshes the workspace preference', async () => {
  vi.mocked(setPreference).mockResolvedValue({ success: true });
  const input = mount('');
  expect(input.labels?.[0]?.textContent).toBe('Public webhook URL');
  edit(input, ' https://hooks.example/notify ');
  await submit();
  expect(setPreference).toHaveBeenCalledWith('webhook_public_url', 'https://hooks.example/notify');
  expect(loadPreferences).toHaveBeenCalledOnce();
});

it('clears the preference to restore Funnel discovery', async () => {
  vi.mocked(deletePreference).mockResolvedValue({ success: true });
  edit(mount('https://hooks.example'), '');
  await submit();
  expect(deletePreference).toHaveBeenCalledWith('webhook_public_url');
  expect(loadPreferences).toHaveBeenCalledOnce();
});

it('shows the engine rejection and keeps the draft for correction', async () => {
  vi.mocked(setPreference).mockRejectedValue(new Error('Enter an HTTPS URL'));
  const input = mount('');
  edit(input, 'http://hooks.example');
  await submit();
  expect(showToast).toHaveBeenCalledWith('Enter an HTTPS URL', 'error');
  expect(input.value).toBe('http://hooks.example');
  expect(loadPreferences).not.toHaveBeenCalled();
});
