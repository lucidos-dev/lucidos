import { beforeEach, expect, it, vi } from 'vitest';
import type { InteractiveDeliveryService } from './interactiveDeliveryService';

const mocks = vi.hoisted(() => ({ idle: undefined as undefined | { preload(): Promise<unknown> }, preload: vi.fn(), install: vi.fn(), toast: vi.fn() }));
vi.mock('../utils/idlePrefetch', () => ({ prefetchWhenIdle: (surface: { preload(): Promise<unknown> }) => { mocks.idle = surface; } }));
vi.mock('../utils/lazyComponent', () => ({ lazyModule: () => ({ preload: mocks.preload, get: () => ({ installInteractiveDelivery: mocks.install }) }) }));
vi.mock('./store', () => ({ showToast: mocks.toast }));
import { installDeferredInteractiveDelivery } from './interactiveDeliveryStartup';
import { submitInteractiveRequest, resolveInteractiveForm, synchronizeInteractiveForms, snapshotInteractiveForms } from './interactiveDeliveryService';

let service: InteractiveDeliveryService;
let cleanup: ReturnType<typeof vi.fn>;
beforeEach(() => {
  vi.clearAllMocks();
  mocks.idle = undefined;
  mocks.preload.mockResolvedValue(true);
  service = {
    submit: vi.fn(async () => {}), resolve: vi.fn(async () => {}), formIds: vi.fn(async () => ['form:open']),
    synchronizeForms: vi.fn(async (body) => body()), reconcileForms: vi.fn(async () => {}),
    publish: vi.fn(async () => {}), reconsider: vi.fn(async () => {}),
  };
  cleanup = vi.fn();
  mocks.install.mockImplementation((register) => { register(service, vi.fn()); return cleanup; });
});

it('retains early requests and resolutions in order without loading before idle', async () => {
  const stop = installDeferredInteractiveDelivery();
  const order: string[] = [];
  vi.mocked(service.submit).mockImplementation(async (r) => { order.push(r.id); });
  vi.mocked(service.resolve).mockImplementation(async (id) => { order.push(id); });
  submitInteractiveRequest({ kind: 'navigation', payload: {}, preference: { threadId: 'source' } }, 'event');
  resolveInteractiveForm('form');
  expect(mocks.preload).not.toHaveBeenCalled();
  await mocks.idle!.preload();
  await vi.waitFor(() => expect(order).toEqual(['event', 'form:form']));
  expect(mocks.install).toHaveBeenCalledOnce();
  stop();
  expect(cleanup).toHaveBeenCalledOnce();
});

it('allows a held form synchronization to call the service after initialization', async () => {
  const stop = installDeferredInteractiveDelivery();
  const body = vi.fn(async () => { expect(await snapshotInteractiveForms()).toEqual(['form:open']); });
  const pending = synchronizeInteractiveForms(body);
  expect(body).not.toHaveBeenCalled();
  await mocks.idle!.preload();
  await pending;
  expect(body).toHaveBeenCalledOnce();
  stop();
});

it('keeps early requests through a failed load and retries without a rejected cached promise', async () => {
  mocks.preload.mockResolvedValueOnce(false);
  const stop = installDeferredInteractiveDelivery();
  submitInteractiveRequest({ kind: 'navigation', payload: {}, preference: { threadId: 'source' } }, 'event');
  await mocks.idle!.preload();
  expect(service.submit).not.toHaveBeenCalled();
  expect(mocks.toast).toHaveBeenCalled();
  const retry = mocks.toast.mock.calls[mocks.toast.mock.calls.length - 1]![2].action.onClick;
  retry();
  await vi.waitFor(() => expect(service.submit).toHaveBeenCalledOnce());
  expect(mocks.preload).toHaveBeenCalledTimes(2);
  stop();
});

it('does not install or dispatch if stopped while the chunk is loading', async () => {
  let finish!: (loaded: boolean) => void;
  mocks.preload.mockImplementationOnce(() => new Promise<boolean>((resolve) => { finish = resolve; }));
  const stop = installDeferredInteractiveDelivery();
  const pending = snapshotInteractiveForms();
  const rejected = expect(pending).rejects.toThrow('Interactive delivery stopped');
  const loading = mocks.idle!.preload();
  stop();
  finish(true);
  await loading;
  await rejected;
  expect(mocks.install).not.toHaveBeenCalled();
  expect(mocks.toast).not.toHaveBeenCalled();
});
