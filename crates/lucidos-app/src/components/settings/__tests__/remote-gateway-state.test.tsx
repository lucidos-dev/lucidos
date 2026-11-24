// @vitest-environment jsdom

import { afterEach, describe, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { RemoteGatewaysSection, remoteGatewayState } from '../GatewayPairingSections';
import * as control from '../../../api/client/control';
import type { Loadable } from '../../../store/types';

vi.mock('../../../api/client/control', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../api/client/control')>()),
  listRemoteGateways: vi.fn(),
  listRemoteWorkspaces: vi.fn(),
}));

const cloud: control.RemoteGateway = {
  name: 'cloud', address: 'https://cloud.example.com', pairing_id: 'pair-cloud',
  gateway_protocol: 5, added_at: '2020-01-01T00:00:00Z', header_names: [], return_access: false,
};
const revoked: control.RemoteGatewayListing = {
  gateway: 'cloud', pairing_id: 'pair-cloud', state: 'unreachable',
  reason: 'cloud unpaired this gateway', reason_kind: 'revoked',
};
const loaded = (data: control.RemoteGatewayListing[]): Loadable<control.RemoteGatewayListing[]> =>
  ({ status: 'loaded', data });

describe('remoteGatewayState', () => {
  it('reads revoked, unreachable and reachable from the listing', () => {
    expect(remoteGatewayState(loaded([revoked]), cloud)).toEqual({ kind: 'revoked' });
    expect(remoteGatewayState(loaded([{ ...revoked, reason: 'timed out', reason_kind: 'timeout' }]), cloud))
      .toEqual({ kind: 'unreachable', reason: 'timed out' });
    expect(remoteGatewayState(loaded([{ gateway: 'cloud', pairing_id: 'pair-cloud', state: 'reachable', workspaces: [] }]), cloud))
      .toEqual({ kind: 'reachable' });
  });

  it('reads unknown for another pairing or a listing not loaded yet', () => {
    expect(remoteGatewayState(loaded([{ ...revoked, pairing_id: 'pair-old' }]), cloud)).toEqual({ kind: 'unknown' });
    expect(remoteGatewayState(loaded([]), cloud)).toEqual({ kind: 'unknown' });
    expect(remoteGatewayState({ status: 'loading' }, cloud)).toEqual({ kind: 'unknown' });
    expect(remoteGatewayState({ status: 'failed', error: 'down' }, cloud)).toEqual({ kind: 'unknown' });
  });
});

describe('RemoteGatewaysSection', () => {
  let host: HTMLDivElement;
  afterEach(() => {
    render(null, host);
    host.remove();
  });

  async function mount() {
    host = document.createElement('div');
    document.body.append(host);
    await act(async () => { render(<RemoteGatewaysSection available />, host); });
    await act(async () => { await Promise.resolve(); });
  }

  const buttons = () => [...host.querySelectorAll('button')].map(b => b.textContent);

  it('offers Re-pair instead of the pairing settings on a revoked row', async () => {
    vi.mocked(control.listRemoteGateways).mockResolvedValue([cloud]);
    vi.mocked(control.listRemoteWorkspaces).mockResolvedValue([revoked]);
    await mount();
    expect(host.textContent).toContain('Access revoked');
    expect(host.querySelector('input[aria-label="New gateway pairing secret from cloud"]')).not.toBeNull();
    expect(buttons()).toContain('Re-pair');
    expect(buttons()).toContain('Remove');
    expect(buttons()).not.toContain('Rotate credential');
  });

  it('draws the rows while the listing is still pending', async () => {
    vi.mocked(control.listRemoteGateways).mockResolvedValue([cloud]);
    vi.mocked(control.listRemoteWorkspaces).mockReturnValue(new Promise(() => {}));
    await mount();
    expect(host.textContent).toContain('https://cloud.example.com');
    expect(buttons()).toContain('Rotate credential');
    expect(host.textContent).not.toContain('Access revoked');
  });

  it('shows a failed listing as an error beside the rows', async () => {
    vi.mocked(control.listRemoteGateways).mockResolvedValue([cloud]);
    vi.mocked(control.listRemoteWorkspaces).mockRejectedValue(new Error('probe failed'));
    await mount();
    expect(host.textContent).toContain('https://cloud.example.com');
    expect(host.textContent).toContain('whether remote gateways still answer');
  });
});
