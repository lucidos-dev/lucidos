// @vitest-environment jsdom

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
import { pairedRemoteGateways, selectSettingsGateway } from '../../../store/actions/settingsGateway';
import type { RemoteGateway } from '../../../api/client/control';
import { GatewayAccessPage, GatewayUsersPage } from '../GatewayGroupPages';

let host: HTMLDivElement;

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
  gatewayUsers.value = { status: 'loaded', data: { users: [], relayed: [] } };
  pairedRemoteGateways.value = { status: 'loaded', data: [{ name: 'cloud' } as RemoteGateway] };
  selectSettingsGateway('cloud');
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

  it('asks to update a remote gateway too old for the hop', () => {
    remoteGatewayDevices.value = { status: 'loaded', data: { refused: 'too-old' } };
    act(() => { render(<RemoteGatewayDevicesSection gateway="cloud" />, host); });
    expect(host.querySelector('[data-role="remote-gateway-too-old"]')?.textContent).toContain('Update cloud');
  });
});

const pressed = () => [...host.querySelectorAll('[aria-pressed="true"]')].map((b) => b.textContent);

describe('the Gateway group in a remote workspace', () => {
  it("opens on the workspace's gateway and shows only its users", () => {
    remoteGatewayUsers.value = {
      status: 'loaded',
      data: { users: [{ id: 'u1', name: 'Sam', emails: [], device_ids: ['b1'] }], relayed: [] },
    };
    act(() => { render(<GatewayUsersPage />, host); });
    expect(pressed()).toEqual(['cloud']);
    expect(titles()).toEqual(['On cloud']);
    expect(host.textContent).toContain('Sam');
  });

  it('shows this gateway once the switcher picks it', () => {
    act(() => { render(<GatewayUsersPage />, host); });
    const thisGateway = [...host.querySelectorAll('button')].find((b) => b.textContent === 'This gateway')!;
    act(() => { thisGateway.click(); });
    expect(pressed()).toEqual(['This gateway']);
    expect(titles()).toEqual(['On this gateway']);
  });

  it('falls back to this gateway when the picked one is no longer paired', () => {
    pairedRemoteGateways.value = { status: 'loaded', data: [] };
    act(() => { render(<GatewayUsersPage />, host); });
    expect(titles()).toEqual(['On this gateway']);
  });

  it('asks to update a remote gateway too old to manage', () => {
    remoteGatewayUsers.value = { status: 'loaded', data: { refused: 'too-old' } };
    act(() => { render(<GatewayUsersPage />, host); });
    expect(host.querySelector('[data-role="remote-gateway-too-old"]')).not.toBeNull();
  });

  it('names the remote gateway when its users fail to load', () => {
    remoteGatewayUsers.value = { status: 'failed', error: 'unreachable' };
    act(() => { render(<GatewayUsersPage />, host); });
    expect(host.querySelector('.error-text')?.textContent).toContain('users on cloud');
  });

  it('shows a failed gateway list and keeps the way back to this gateway', () => {
    pairedRemoteGateways.value = { status: 'failed', error: 'unreachable' };
    act(() => { render(<GatewayUsersPage />, host); });
    expect(host.querySelector('.error-text')?.textContent).toContain('remote gateways');
    expect([...host.querySelectorAll('button')].map((b) => b.textContent)).toContain('This gateway');
  });

  it('draws no remote Access data, only where to open it', () => {
    act(() => { render(<GatewayAccessPage />, host); });
    expect(host.querySelector('[data-role="remote-gateway-page-notice"]')?.textContent)
      .toContain('Settings → Access on cloud');
  });
});
