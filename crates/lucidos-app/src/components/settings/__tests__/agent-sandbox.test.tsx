// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

vi.mock('../../../api/client', async (original) => ({
  ...(await original<typeof import('../../../api/client')>()),
  setPreference: vi.fn(async () => {}),
}));
vi.mock('../../../store/store', async (original) => ({
  ...(await original<typeof import('../../../store/store')>()),
  showConfirm: vi.fn(async () => true),
}));

import { SettingsView } from '../SettingsView';
import { preferences, settingsSubview, showConfirm } from '../../../store/store';
import { setPreference } from '../../../api/client';

let host: HTMLDivElement;
beforeEach(() => {
  vi.clearAllMocks();
  host = document.createElement('div');
  document.body.appendChild(host);
  settingsSubview.value = 'permissions';
});
afterEach(() => {
  render(null, host);
  host.remove();
  settingsSubview.value = 'main';
  preferences.value = { status: 'not-loaded' };
});

async function mount(unsandboxed?: string) {
  preferences.value = { status: 'loaded', data: unsandboxed === undefined ? {} : { agents_unsandboxed: unsandboxed } };
  await act(() => { render(<SettingsView />, host); });
  return host.querySelector<HTMLInputElement>('[data-search-anchor="agent-sandbox:unsandboxed"] input')!;
}

it.each([undefined, 'false'])('shows Sandbox agents ON for the safe default %s', async (value) => {
  const toggle = await mount(value);
  expect(toggle.checked).toBe(true);
  expect(toggle.getAttribute('aria-label')).toBe('Sandbox agents');
});

it('writes agents_unsandboxed=true when the owner turns Sandbox agents OFF', async () => {
  const toggle = await mount('false');
  await act(() => { toggle.click(); });
  expect(showConfirm).toHaveBeenCalledOnce();
  await vi.waitFor(() => expect(setPreference).toHaveBeenCalledWith('agents_unsandboxed', 'true', undefined));
});

it('keeps sandboxing when the owner cancels disabling it', async () => {
  vi.mocked(showConfirm).mockResolvedValueOnce(false);
  const toggle = await mount('false');
  await act(() => { toggle.click(); });
  expect(setPreference).not.toHaveBeenCalled();
  await vi.waitFor(() => expect(toggle.checked).toBe(true));
});

it('shows OFF for unsandboxed agents and writes false when switched ON', async () => {
  const toggle = await mount('true');
  expect(toggle.checked).toBe(false);
  await act(() => { toggle.click(); });
  expect(showConfirm).not.toHaveBeenCalled();
  expect(setPreference).toHaveBeenCalledWith('agents_unsandboxed', 'false', undefined);
});
