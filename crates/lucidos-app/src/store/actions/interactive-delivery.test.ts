import { beforeEach, expect, it, vi } from 'vitest';
import type { InteractiveDeliveryService } from '../interactiveDeliveryService';

const mocks = vi.hoisted(() => {
  const coordinator = {
    formIds: vi.fn(async () => []), synchronizeForms: vi.fn(async (body: () => Promise<void>) => body()),
    reconcileForms: vi.fn(async () => {}), submit: vi.fn(async () => {}), resolve: vi.fn(async () => {}),
    publish: vi.fn(async () => {}), reconsider: vi.fn(async () => {}), retry: vi.fn(async () => {}), stop: vi.fn(async () => {}),
  };
  return { preload: vi.fn(async () => true), toast: vi.fn(), coordinator, create: vi.fn(() => coordinator), install: vi.fn((_service: InteractiveDeliveryService) => vi.fn()) };
});
vi.mock('../../utils/lazyComponent', () => ({ lazyModule: (load: () => Promise<unknown>) => {
  let loaded: unknown;
  return { preload: async () => { if (!await mocks.preload()) return false; loaded = await load(); return true; }, get: () => loaded };
} }));
vi.mock('../interactiveDeliveryDevice', () => ({ createDeviceInteractiveDelivery: mocks.create }));
vi.mock('../interactiveDeliveryService', () => ({ installInteractiveDeliveryService: mocks.install }));
vi.mock('../store', async () => {
  const { signal } = await import('@preact/signals');
  return { activeInlineForm: signal(null), confirmState: signal({ visible: false }), ensureAskDialogsReady: vi.fn(), showConfirm: vi.fn(), showToast: mocks.toast };
});
vi.mock('./form-requests', () => ({ presentFormRequest: vi.fn() }));
vi.mock('./navigation-request', () => ({ presentThreadNavigation: vi.fn() }));
vi.mock('./oauth', () => ({ presentOAuthAccountConnected: vi.fn() }));
vi.mock('./devices', () => ({ getDeviceId: () => 'test-device' }));
vi.mock('./push', () => ({ setDevicePushEnabled: vi.fn() }));
import { installInteractiveDelivery, presentInteractiveRequest } from './interactive-delivery';

beforeEach(() => { vi.clearAllMocks(); });

it('closing a form or confirm publishes again, so pending work retries without a new connection', async () => {
  const activeInlineForm = (await import('../store')).activeInlineForm as unknown as { value: unknown };
  const stop = installInteractiveDelivery();
  try {
    await vi.waitFor(() => expect(mocks.coordinator.publish).toHaveBeenCalled());
    mocks.coordinator.publish.mockClear();
    activeInlineForm.value = { id: 'form' } as never;
    activeInlineForm.value = null;
    await vi.waitFor(() => expect(mocks.coordinator.publish).toHaveBeenCalledTimes(2));
  } finally { stop(); await vi.waitFor(() => expect(mocks.coordinator.stop).toHaveBeenCalledOnce()); }
});

it('opens the push prompt at once, and asks for the permission only after the answer', async () => {
  const { showConfirm } = await import('../store');
  const { setDevicePushEnabled } = await import('./push');
  let answer!: (ok: boolean) => void;
  vi.mocked(showConfirm).mockReturnValueOnce(new Promise<boolean>((resolve) => { answer = resolve; }) as never);
  expect(presentInteractiveRequest({ id: 'push', kind: 'push-enable', payload: {}, preference: { threadId: 'thread' } })).toBe(true);
  expect(showConfirm).toHaveBeenCalledTimes(1);
  expect(setDevicePushEnabled).not.toHaveBeenCalled();
  answer(true);
  await vi.waitFor(() => expect(setDevicePushEnabled).toHaveBeenCalledWith('test-device', true));
});

it('installs the service synchronously and waits for the adapter before submitting', async () => {
  const stop = installInteractiveDelivery();
  try {
    const service = mocks.install.mock.calls[0][0];
    const request = { id: 'request', kind: 'navigation' as const, payload: {}, preference: { threadId: 'thread' } };
    const pending = service.submit(request);
    expect(mocks.coordinator.submit).not.toHaveBeenCalled();
    await pending;
    expect(mocks.coordinator.submit).toHaveBeenCalledWith(request);
  } finally { stop(); await vi.waitFor(() => expect(mocks.coordinator.stop).toHaveBeenCalledOnce()); }
});

it('stops an adapter loaded after teardown without publishing or submitting', async () => {
  const stop = installInteractiveDelivery();
  const service = mocks.install.mock.calls[0][0];
  const pending = service.submit({ id: 'request', kind: 'navigation', payload: {}, preference: { threadId: 'thread' } });
  stop();
  await expect(pending).rejects.toThrow('Interactive delivery stopped');
  await vi.waitFor(() => expect(mocks.coordinator.stop).toHaveBeenCalledOnce());
  expect(mocks.coordinator.submit).not.toHaveBeenCalled();
  expect(mocks.coordinator.publish).not.toHaveBeenCalled();
});

it('retries loading after a chunk failure instead of retaining a rejected promise', async () => {
  mocks.preload.mockResolvedValueOnce(false);
  const stop = installInteractiveDelivery();
  try {
    await vi.waitFor(() => expect(mocks.toast).toHaveBeenCalled());
    const service = mocks.install.mock.calls[0][0];
    const request = { id: 'request', kind: 'navigation' as const, payload: {}, preference: { threadId: 'thread' } };
    await service.submit(request);
    expect(mocks.coordinator.submit).toHaveBeenCalledWith(request);
    expect(mocks.preload.mock.calls.length).toBeGreaterThan(1);
  } finally { stop(); await vi.waitFor(() => expect(mocks.coordinator.stop).toHaveBeenCalledOnce()); }
});

it('reports adapter cleanup failures after teardown', async () => {
  const stop = installInteractiveDelivery();
  await mocks.install.mock.calls[0][0].publish();
  mocks.coordinator.stop.mockRejectedValueOnce(new Error('Storage refused'));
  stop();
  await vi.waitFor(() => expect(mocks.toast).toHaveBeenCalledWith('Interactive delivery cleanup failed: Storage refused', 'error'));
});
