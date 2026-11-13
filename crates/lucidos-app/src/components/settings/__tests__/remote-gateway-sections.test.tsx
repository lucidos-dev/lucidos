// @vitest-environment jsdom

// A remote workspace shows the remote gateway's lists under "On <name>". A
// refused grant says how to turn it on. A gateway too old for the hop hides them.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

vi.mock('../../../utils/basePath', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../utils/basePath')>()),
  REMOTE_GATEWAY: 'cloud',
}));

import { remoteGatewayDevices, remoteGatewayUsers } from '../../../store/actions/remoteGatewayManage';
import { gatewayUsers } from '../../../store/actions/gatewayUsers';
import { RemoteGatewayDevicesSection } from '../remoteGatewaySections';
import { UsersPage } from '../UsersSection';

let host: HTMLDivElement;

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
  gatewayUsers.value = { status: 'loaded', data: { users: [], relayed: [] } };
});

afterEach(() => {
  render(null, host);
  host.remove();
  remoteGatewayDevices.value = { status: 'not-loaded' };
  remoteGatewayUsers.value = { status: 'not-loaded' };
});

const titles = () => [...host.querySelectorAll('.settings-section-title')].map((t) => t.textContent);

describe('remote gateway devices', () => {
  it("lists the remote gateway's devices under its name, each with Revoke", () => {
    remoteGatewayDevices.value = {
      status: 'loaded',
      data: [{ id: 'b1', label: 'Office laptop', paired_at: '2026-01-01T00:00:00Z' }],
    };
    act(() => { render(<RemoteGatewayDevicesSection gateway="cloud" />, host); });
    expect(titles()).toEqual(['On cloud']);
    expect(host.textContent).toContain('Office laptop');
    expect(host.querySelector('.action-btn-danger')?.textContent).toContain('Revoke');
  });

  it('says how to turn the grant on when the remote gateway refuses', () => {
    remoteGatewayDevices.value = { status: 'loaded', data: { refused: 'not-granted' } };
    act(() => { render(<RemoteGatewayDevicesSection gateway="cloud" />, host); });
    expect(host.querySelector('[data-role="manage-not-granted"]')?.textContent).toContain('Pair a gateway');
  });

  it('hides the section for a remote gateway too old for the hop', () => {
    remoteGatewayDevices.value = { status: 'loaded', data: { refused: 'too-old' } };
    act(() => { render(<RemoteGatewayDevicesSection gateway="cloud" />, host); });
    expect(host.textContent).toBe('');
  });
});

describe('the Users page in a remote workspace', () => {
  it("shows the remote gateway's users first, then this gateway's", () => {
    remoteGatewayUsers.value = {
      status: 'loaded',
      data: { users: [{ id: 'u1', name: 'Sam', emails: [], device_ids: ['b1'] }], relayed: [] },
    };
    act(() => { render(<UsersPage />, host); });
    expect(titles()).toEqual(['On cloud', 'On this gateway']);
    expect(host.textContent).toContain('Sam');
  });

  it("keeps this gateway's users when the remote gateway is too old", () => {
    remoteGatewayUsers.value = { status: 'loaded', data: { refused: 'too-old' } };
    act(() => { render(<UsersPage />, host); });
    expect(titles()).toEqual(['On this gateway']);
  });

  it('names the remote gateway when its users fail to load', () => {
    remoteGatewayUsers.value = { status: 'failed', error: 'unreachable' };
    act(() => { render(<UsersPage />, host); });
    expect(host.querySelector('.error-text')?.textContent).toContain('users on cloud');
  });
});
