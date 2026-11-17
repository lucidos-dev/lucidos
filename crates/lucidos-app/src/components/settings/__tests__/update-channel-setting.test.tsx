// @vitest-environment jsdom
import { it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
const mocks = vi.hoisted(() => ({ save: vi.fn(), toast: vi.fn() }));
vi.mock('../../../api/client/control', async (original) => ({
  ...(await original<typeof import('../../../api/client/control')>()),
  setReleaseCheckConfig: mocks.save,
}));
vi.mock('../../../store/store', async (original) => ({
  ...(await original<typeof import('../../../store/store')>()),
  showToast: mocks.toast,
}));
import { clientVersionLabel } from '../../../utils/clientVersion';
import { UpdateChannelNotices, UpdateChannelSetting } from '../UpdateChannelSetting';
import { releaseCheck } from '../../../store/store';
import type { ReleaseCheck } from '../../../api/client/control';
const custom = [{ name: 'staging', url: 'https://example.com/latest.json' }];
function gateway(over: Partial<ReleaseCheck> = {}): ReleaseCheck {
  return { enabled: true, supported: true, current_version: '9.2.0', checked_at: null,
    last_error: null, latest: null, update_address: null, update_address_error: null,
    channel: 'beta', channels: [
      { channel: 'stable', available: true, reason: null },
      { channel: 'beta', available: true, reason: null },
      { channel: 'nightly', available: false, reason: 'Not published' },
      { channel: 'local', available: false, reason: 'No local feed' },
    ], custom, channel_error: null, ahead_of_channel: false, channel_version: null, ...over };
}
let host: HTMLElement;
const button = (s: string) => [...host.querySelectorAll('button')].find(b => b.textContent?.trim() === s)!;
beforeEach(() => { host = document.createElement('div'); document.body.append(host); mocks.save.mockReset(); mocks.toast.mockReset(); mocks.save.mockResolvedValue(gateway()); releaseCheck.value = null; });
afterEach(() => { render(null, host); host.remove(); });
const draw = (c = gateway()) => act(() => render(<UpdateChannelSetting check={c} />, host));
it('selects named channels without echoing or losing custom slots', async () => {
  draw(); expect(button('Beta').getAttribute('aria-pressed')).toBe('true');
  act(() => button('Stable').click());
  await vi.waitFor(() => expect(mocks.save).toHaveBeenCalledWith({ channel: 'stable' }));
  expect(releaseCheck.value?.custom).toEqual(custom);
});
it('omits Nightly and Local without a usable feed', () => {
  draw(); expect(button('Nightly')).toBeUndefined(); expect(button('Local')).toBeUndefined();
});
it('hides the control for older gateways and gateways with no non-Stable choice', () => {
  draw(gateway({ channels: undefined })); expect(host.textContent).toBe('');
  draw(gateway({ channels: [{ channel: 'stable', available: true, reason: null }] })); expect(host.textContent).toBe('');
});
it('offers Local only when available', () => {
  draw(gateway({ channels: [...gateway().channels!, { channel: 'local', available: true, reason: null }] }));
  expect(button('Local')).toBeDefined();
});
it('defaults custom addresses to collapsed and rolls the fold', () => {
  draw(); const toggle = host.querySelector<HTMLButtonElement>('.settings-disclosure-toggle')!;
  expect(toggle.getAttribute('aria-expanded')).toBe('false');
  act(() => toggle.click()); expect(toggle.getAttribute('aria-expanded')).toBe('true');
});
it('selects and removes a custom slot with a partial write', async () => {
  draw(); act(() => host.querySelector<HTMLButtonElement>('.settings-disclosure-toggle')!.click());
  act(() => button('Select').click());
  await vi.waitFor(() => expect(mocks.save).toHaveBeenCalledWith({ channel: 'custom:staging' }));
  await vi.waitFor(() => expect(button('Remove').disabled).toBe(false));
  act(() => button('Remove').click());
  await vi.waitFor(() => expect(mocks.save).toHaveBeenCalledWith({ custom_remove: 'staging' }));
});
it('adds a trimmed named address and clears only a successful draft', async () => {
  draw(); act(() => host.querySelector<HTMLButtonElement>('.settings-disclosure-toggle')!.click());
  for (const [label, value] of [['Name', ' test '], ['Address', ' https://example.com/test/latest.json ']]) {
    const input = host.querySelector<HTMLInputElement>(`[aria-label="${label}"]`)!;
    act(() => { input.value = value; input.dispatchEvent(new Event('input', { bubbles: true })); });
  }
  act(() => button('Add address').click());
  await vi.waitFor(() => expect(mocks.save).toHaveBeenCalledWith({ custom_upsert: { name: 'test', url: 'https://example.com/test/latest.json' } }));
  await vi.waitFor(() => expect(host.querySelector<HTMLInputElement>('[aria-label="Name"]')!.value).toBe(''));
});
it('does not draw a persistent non-Stable banner', () => {
  render(<UpdateChannelNotices check={gateway()} />, host); expect(host.textContent).toBe('');
});
it('names both versions when ahead, but reports a feed failure instead of stale success', () => {
  render(<UpdateChannelNotices check={gateway({ ahead_of_channel: true, channel_version: '9.1.0' })} />, host);
  expect(host.textContent).toContain('ahead of Beta'); expect(host.textContent).toContain('9.2.0'); expect(host.textContent).toContain('9.1.0');
  render(<UpdateChannelNotices check={gateway({ channel_error: 'Feed unavailable', ahead_of_channel: true, channel_version: '9.1.0' })} />, host);
  expect(host.textContent).toContain('Feed unavailable'); expect(host.textContent).not.toContain('ahead');
});
it('shows a failed gateway poll without treating it as a channel refusal', () => {
  render(<UpdateChannelNotices check={gateway({ channel: 'stable', last_error: 'Check origin unavailable' })} />, host);
  expect(host.textContent).toContain('Check origin unavailable');
  expect(host.textContent).not.toContain('Back to Stable');
});
it('offers a return to Stable on a failed channel', async () => {
  render(<UpdateChannelNotices check={gateway({ channel_error: 'Feed unavailable' })} />, host);
  button('Back to Stable').click();
  await vi.waitFor(() => expect(mocks.save).toHaveBeenCalledWith({ channel: 'stable' }));
});

it('removing the selected slot atomically returns to Stable', async () => {
  draw(gateway({ channel: 'custom:staging' }));
  act(() => host.querySelector<HTMLButtonElement>('.settings-disclosure-toggle')!.click());
  act(() => button('Remove').click());
  await vi.waitFor(() => expect(mocks.save).toHaveBeenCalledWith({ custom_remove: 'staging', channel: 'stable' }));
});

it('keeps a rejected draft and reports the gateway reason', async () => {
  mocks.save.mockRejectedValue(new Error('Invalid custom address'));
  draw();
  act(() => host.querySelector<HTMLButtonElement>('.settings-disclosure-toggle')!.click());
  for (const [label, value] of [['Name', 'test'], ['Address', 'https://example.com/test/latest.json']]) {
    const input = host.querySelector<HTMLInputElement>(`[aria-label="${label}"]`)!;
    act(() => { input.value = value; input.dispatchEvent(new Event('input', { bubbles: true })); });
  }
  act(() => button('Add address').click());
  await vi.waitFor(() => expect(mocks.toast).toHaveBeenCalledWith(expect.stringContaining('Invalid custom address'), 'error'));
  expect(host.querySelector<HTMLInputElement>('[aria-label="Name"]')!.value).toBe('test');
});
it('client identity reads the live channel beside the client version', () => {
  window.__LUCIDOS_APP_VERSION__ = '9.2.0';
  try {
    releaseCheck.value = gateway();
    expect(clientVersionLabel()).toBe('9.2.0 · Beta');
    releaseCheck.value = gateway({ channel: 'stable' });
    expect(clientVersionLabel()).toBe('9.2.0 · Stable');
  } finally {
    delete window.__LUCIDOS_APP_VERSION__;
  }
});
