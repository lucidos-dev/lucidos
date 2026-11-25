// @vitest-environment jsdom

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { RemoteGatewaysSection } from '../GatewayPairingSections';
import * as control from '../../../api/client/control';
import * as store from '../../../store/store';
import { openFold } from './openFold';

vi.mock('../../../api/client/control', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../api/client/control')>()),
  listRemoteGateways: vi.fn(),
  listRemoteWorkspaces: vi.fn(),
  setRemoteGatewayTrustedPairing: vi.fn(),
}));

vi.mock('../../../store/store', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/store')>()),
  showConfirm: vi.fn(),
  showToast: vi.fn(),
}));

const cloud: control.RemoteGateway = {
  name: 'cloud', address: 'https://cloud.example.com', pairing_id: 'pair-cloud',
  gateway_protocol: 5, added_at: '2020-01-01T00:00:00Z', header_names: [],
  return_access: false, trusted_pairing: false,
};
const LABEL = "Use cloud's own app for its workspaces";

describe('trusted pairing toggle', () => {
  let host: HTMLDivElement;
  beforeEach(() => {
    vi.mocked(control.listRemoteWorkspaces).mockReturnValue(new Promise(() => {}));
    vi.mocked(control.setRemoteGatewayTrustedPairing).mockResolvedValue({ ...cloud, trusted_pairing: true });
  });
  afterEach(() => {
    render(null, host);
    host.remove();
    vi.clearAllMocks();
  });

  async function toggle(gateway: control.RemoteGateway) {
    vi.mocked(control.listRemoteGateways).mockResolvedValue([gateway]);
    host = document.createElement('div');
    document.body.append(host);
    await act(async () => { render(<RemoteGatewaysSection available />, host); });
    await act(async () => { await Promise.resolve(); });
    await act(() => { openFold(host, 'cloud'); });
    const input = host.querySelector<HTMLInputElement>(`input[aria-label="${LABEL}"]`)!;
    expect(input.checked).toBe(gateway.trusted_pairing);
    await act(async () => { input.click(); });
    await act(async () => { await Promise.resolve(); });
  }

  it('asks before trusting, and changes nothing when declined', async () => {
    vi.mocked(store.showConfirm).mockResolvedValue(false);
    await toggle(cloud);
    expect(store.showConfirm).toHaveBeenCalledOnce();
    expect(control.setRemoteGatewayTrustedPairing).not.toHaveBeenCalled();
  });

  it('turns it on after the owner confirms', async () => {
    vi.mocked(store.showConfirm).mockResolvedValue(true);
    await toggle(cloud);
    expect(control.setRemoteGatewayTrustedPairing).toHaveBeenCalledWith('cloud', true);
  });

  it('turns it off without asking', async () => {
    await toggle({ ...cloud, trusted_pairing: true });
    expect(store.showConfirm).not.toHaveBeenCalled();
    expect(control.setRemoteGatewayTrustedPairing).toHaveBeenCalledWith('cloud', false);
  });
});
