// @vitest-environment jsdom
/**
 * The update address must be impossible to miss while set, and one click must
 * clear it (plan `docs/plans/2026-10-21-update-address-setting.md`).
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';

const mocks = vi.hoisted(() => ({ setReleaseCheckConfig: vi.fn() }));

vi.mock('../../../api/client/control', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../api/client/control')>()),
  setReleaseCheckConfig: mocks.setReleaseCheckConfig,
}));

import { UpdateAddressBanner, UpdateAddressRow } from '../UpdateAddressSetting';
import { releaseCheck } from '../../../store/store';
import type { ReleaseCheck } from '../../../api/client/control';

const RC = 'http://127.0.0.1:5300/api/v1/data/artifacts/releases/rc/latest.json';

function gateway(over: Partial<ReleaseCheck> = {}): ReleaseCheck {
  return {
    enabled: true,
    supported: true,
    current_version: '0.50.0',
    checked_at: null,
    last_error: null,
    latest: null,
    update_address: null,
    update_address_error: null,
    ...over,
  };
}

describe('the update address in Settings', () => {
  let host: HTMLElement;

  beforeEach(() => {
    host = document.createElement('div');
    document.body.appendChild(host);
    mocks.setReleaseCheckConfig.mockReset();
    mocks.setReleaseCheckConfig.mockResolvedValue(gateway());
    releaseCheck.value = null;
  });

  afterEach(() => {
    render(null, host);
    host.remove();
  });

  const banner = () => host.querySelector('[data-role="update-address-banner"]');
  const button = (label: string) =>
    [...host.querySelectorAll('button')].find((b) => b.textContent?.trim() === label);

  it('names the address it overrides with', () => {
    render(<UpdateAddressBanner check={gateway({ update_address: RC })} />, host);
    expect(banner()?.textContent).toContain('Update address overridden.');
    expect(banner()?.querySelector('code')?.textContent).toBe(RC);
  });

  it('draws no banner when no address is set', () => {
    render(<UpdateAddressBanner check={gateway()} />, host);
    expect(banner()).toBeNull();
  });

  it('says a refused address is refused, and that checks fail', () => {
    const error = 'the update address must be a latest.json URL (scheme ftp is not allowed): ftp://x';
    render(
      <UpdateAddressBanner check={gateway({ update_address: 'ftp://x', update_address_error: error })} />,
      host,
    );
    expect(banner()?.textContent).toContain('Update address refused.');
    expect(banner()?.textContent).toContain(error);
  });

  it('Use default sends the empty address and takes the answer', async () => {
    render(<UpdateAddressBanner check={gateway({ update_address: RC })} />, host);
    button('Use default')?.click();
    await vi.waitFor(() => expect(releaseCheck.value).not.toBeNull());
    expect(mocks.setReleaseCheckConfig).toHaveBeenCalledWith({ update_address: '' });
  });

  it('Save sends the typed address, trimmed', async () => {
    render(<UpdateAddressRow stored="" />, host);
    const save = button('Save');
    expect(save?.disabled).toBe(true);
    const input = host.querySelector('input') as HTMLInputElement;
    input.value = `  ${RC} `;
    input.dispatchEvent(new Event('input', { bubbles: true }));
    await vi.waitFor(() => expect(button('Save')?.disabled).toBe(false));
    button('Save')?.click();
    await vi.waitFor(() =>
      expect(mocks.setReleaseCheckConfig).toHaveBeenCalledWith({ update_address: RC }),
    );
  });
});
