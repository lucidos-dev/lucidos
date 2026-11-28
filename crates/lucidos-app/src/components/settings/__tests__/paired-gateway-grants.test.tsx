// @vitest-environment jsdom

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import type { PairedGateway } from '../../../api/client/control';

const paired: PairedGateway = {
  id: 'pairing-1',
  label: 'Gateway A',
  paired_at: '2026-01-01T00:00:00Z',
  last_used_at: null,
  may_manage: true,
  may_control: false,
  may_admin: false,
  remote_starts: 'deny',
};

const { setGrant, mint, listed } = vi.hoisted(() => ({
  setGrant: vi.fn(async (_id: string, _grant: string, _on: boolean): Promise<unknown> => undefined),
  mint: vi.fn(async (_grants: readonly string[]) => ({ code: 'secret', expires_in_secs: 600 })),
  listed: { rows: [] as PairedGateway[] },
}));

vi.mock('../../../api/client/control', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../api/client/control')>()),
  listPairedGateways: vi.fn(async () => listed.rows),
  setPairedGatewayGrant: setGrant,
  mintGatewayPairingCode: mint,
}));

vi.mock('../../../store/store', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/store')>()),
  showConfirm: vi.fn(async () => true),
}));

import { PairGatewaySection, grantsDetail } from '../GatewayPairingSections';

let host: HTMLDivElement;

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
  setGrant.mockClear();
  mint.mockClear();
  listed.rows = [paired];
});

afterEach(() => {
  render(null, host);
  host.remove();
});

const settle = () => act(async () => { await new Promise((r) => setTimeout(r, 0)); });

describe('paired gateway grants', () => {
  it('names each grant the pairing carries', () => {
    expect(grantsDetail(paired)).toBe(
      "can open this gateway's workspaces · may manage its users and devices",
    );
    expect(grantsDetail({ ...paired, may_manage: false, may_control: true })).toBe(
      "can open this gateway's workspaces · may create and restore its workspaces",
    );
  });

  it('turns the control grant on from its own toggle, beside the manage one', async () => {
    act(() => { render(<PairGatewaySection available target={{ kind: 'none' }} />, host); });
    await settle();
    const control = host.querySelector<HTMLInputElement>(
      'input[aria-label="Let devices on Gateway A create, start, stop and restore workspaces"]',
    );
    const manage = host.querySelector<HTMLInputElement>(
      'input[aria-label="Let devices on Gateway A manage users and devices"]',
    );
    expect(manage?.checked).toBe(true);
    expect(control?.checked).toBe(false);
    control!.checked = true;
    act(() => { control!.dispatchEvent(new Event('change', { bubbles: true })); });
    await settle();
    expect(setGrant).toHaveBeenCalledWith('pairing-1', 'control', true);
  });

  it('names only the admin grant once it includes the other two', () => {
    expect(grantsDetail({ ...paired, may_admin: true })).toBe(
      "can open this gateway's workspaces · may manage it as a device paired here",
    );
  });

  it('locks manage and control on while the pairing holds admin', async () => {
    listed.rows = [{ ...paired, may_manage: false, may_admin: true }];
    act(() => { render(<PairGatewaySection available target={{ kind: 'none' }} />, host); });
    await settle();
    const manage = host.querySelector<HTMLInputElement>(
      'input[aria-label="Let devices on Gateway A manage users and devices"]',
    );
    const admin = host.querySelector<HTMLInputElement>(
      'input[aria-label="Let devices on Gateway A manage this gateway as a device paired here"]',
    );
    expect(admin?.checked).toBe(true);
    expect(manage?.checked).toBe(true);
    expect(manage?.disabled).toBe(true);
  });

  it('mints the secret with the grants ticked first', async () => {
    listed.rows = [];
    act(() => { render(<PairGatewaySection available target={{ kind: 'none' }} />, host); });
    await settle();
    const admin = host.querySelector<HTMLInputElement>(
      'input[aria-label="Let devices on the other gateway manage this gateway as a device paired here"]',
    );
    admin!.checked = true;
    act(() => { admin!.dispatchEvent(new Event('change', { bubbles: true })); });
    await settle();
    const control = host.querySelector<HTMLInputElement>(
      'input[aria-label="Let devices on the other gateway create, start, stop and restore workspaces"]',
    );
    expect(control?.checked).toBe(true);
    expect(control?.disabled).toBe(true);
    const create = [...host.querySelectorAll('button')].find((b) => b.textContent === 'Create a gateway secret');
    act(() => { create!.click(); });
    await settle();
    expect(mint).toHaveBeenCalledWith(['admin']);
  });
});
