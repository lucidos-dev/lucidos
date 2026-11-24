// @ts-expect-error Node APIs are available in Vitest, outside the app type environment.
import { BroadcastChannel as NodeBroadcastChannel, receiveMessageOnPort } from 'node:worker_threads';
import { expect, test } from 'vitest';
import { ensureNavigator } from './test-navigator';

test('BroadcastChannel reaches peers in the test file without reaching the unscoped channel', () => {
  const name = 'vitest-broadcast-channel-isolation';
  const sender = new BroadcastChannel(name);
  const peer = new BroadcastChannel(name);
  const unscoped = new NodeBroadcastChannel(name);

  try {
    sender.postMessage('peer message');

    expect(receiveMessageOnPort(unscoped)).toBeUndefined();
    expect(receiveMessageOnPort(peer)).toEqual({ message: 'peer message' });
  } finally {
    sender.close();
    peer.close();
    unscoped.close();
  }
});

test('a host without a navigator global gets one, and a host with one keeps it', () => {
  const bare: { navigator?: { userAgent: string } } = {};
  ensureNavigator(bare);
  expect(bare.navigator?.userAgent).toBe('node');

  const real = { userAgent: 'Mozilla/5.0' };
  const host = { navigator: real };
  ensureNavigator(host);
  expect(host.navigator).toBe(real);
});
