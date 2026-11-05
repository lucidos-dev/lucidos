import { describe, expect, it, vi } from 'vitest';
import {
  EMPTY_REMOTE_GATEWAY_DRAFT,
  remoteGatewayBlocker,
  shortNameInput,
  submitRemoteGateway,
} from './remoteGatewayForm';
import { gatewayPairingAddress } from './GatewayPairingSections';

import { addRemoteGateway } from '../../api/client/control';
vi.mock('../../api/client/control', async original => ({ ...await original<typeof import('../../api/client/control')>(), addRemoteGateway: vi.fn() }));

const READY = { address: 'https://cloud.example.com', code: '12345678', name: 'cloud' };

describe('remoteGatewayBlocker', () => {
  it('holds an incomplete draft', () => {
    expect(remoteGatewayBlocker(EMPTY_REMOTE_GATEWAY_DRAFT)).toEqual({ kind: 'incomplete' });
    expect(remoteGatewayBlocker({ ...READY, name: '  ' })).toEqual({ kind: 'incomplete' });
  });

  it('names plain http before the gateway would refuse it', () => {
    expect(remoteGatewayBlocker({ ...READY, address: 'http://cloud.example.com' })).toEqual({
      kind: 'plain-http',
    });
    expect(remoteGatewayBlocker({ address: 'HTTP://x', code: '', name: '' })).toEqual({
      kind: 'plain-http',
    });
  });

  it('lets a complete https draft through, leaving the rest to the gateway', () => {
    expect(remoteGatewayBlocker(READY)).toBeNull();
  });
});

describe('shortNameInput', () => {
  it('lowercases as the user types, since routes are lowercase', () => {
    expect(shortNameInput('Cloud')).toBe('cloud');
  });
});

describe('gatewayPairingAddress', () => {
  it('offers only an https pairing origin', () => {
    expect(gatewayPairingAddress({ kind: 'origin', url: 'https://mac.example.ts.net' })).toBe(
      'https://mac.example.ts.net',
    );
    expect(gatewayPairingAddress({ kind: 'origin', url: 'http://10.0.0.2:5252' })).toBeNull();
    expect(gatewayPairingAddress({ kind: 'none' })).toBeNull();
    expect(gatewayPairingAddress({ kind: 'resolving' })).toBeNull();
  });
});


it('sends proxy headers with the initial pairing request, preserving secret whitespace', () => {
  submitRemoteGateway({ ...READY, headers: [{ name: 'Proxy-Token', value: ' secret ' }] });
  expect(addRemoteGateway).toHaveBeenCalledWith({ ...READY, headers: { 'proxy-token': ' secret ' } });
});
