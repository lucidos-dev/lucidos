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
  remote_starts: 'deny',
};

const { setGrant } = vi.hoisted(() => ({
  setGrant: vi.fn(async (_id: string, _grant: string, _on: boolean): Promise<unknown> => undefined),
}));

vi.mock('../../../api/client/control', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../api/client/control')>()),
  listPairedGateways: vi.fn(async () => [paired]),
  setPairedGatewayGrant: setGrant,
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
});
