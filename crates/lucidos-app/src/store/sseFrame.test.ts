import { expect, it, vi } from 'vitest';
import { dispatchSSEFrame } from './sseFrame';

function handlers() {
  return { thread: vi.fn(), system: vi.fn(), references: vi.fn() };
}

it('preserves thread identity for direct and relayed legacy frames', () => {
  const receivers = handlers();
  const payload = { thread_id: 'thread', event_id: 'request', event: { type: 'NavigationRequested' } };
  const frame = { type: 'ThreadEvent', data: payload };
  dispatchSSEFrame(frame, receivers);
  dispatchSSEFrame(JSON.parse(JSON.stringify(frame)), receivers);
  expect(receivers.thread).toHaveBeenNthCalledWith(1, payload, 'request');
  expect(receivers.thread).toHaveBeenNthCalledWith(2, payload, 'request');
  expect(receivers.references).toHaveBeenCalledWith('ThreadEvent', payload, 'request');
});

it('keeps system envelope identity separate from payload event_id', () => {
  const receivers = handlers();
  const payload = { event_id: 'notification-target', title: 'Notice' };
  dispatchSSEFrame({ type: 'NotificationCreated', event_id: 'emission', data: payload }, receivers);
  expect(receivers.system).toHaveBeenCalledWith('NotificationCreated', payload, 'emission');
  expect(receivers.references).toHaveBeenCalledWith('NotificationCreated', payload, 'emission');
  expect(payload.event_id).toBe('notification-target');
});

it('does not collapse distinct events with identical payloads or invent legacy identity', () => {
  const receivers = handlers();
  const data = { provider: 'example' };
  for (const event_id of ['first', 'first', 'second']) {
    dispatchSSEFrame({ type: 'OAuthAccountConnected', event_id, data }, receivers);
  }
  expect(receivers.references.mock.calls.map((call) => call[2])).toEqual(['first', 'first', 'second']);
  dispatchSSEFrame({ type: 'NotificationCreated', data: { event_id: 'target' } }, receivers);
  expect(receivers.references.mock.lastCall?.[2]).toBeUndefined();
});
