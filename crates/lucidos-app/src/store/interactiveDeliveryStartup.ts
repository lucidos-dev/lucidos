/** Register before SSE connects; hold calls until delivery can load after paint. */
import { installInteractiveDeliveryService, type InteractiveDeliveryService } from './interactiveDeliveryService';
import { prefetchWhenIdle } from '../utils/idlePrefetch';
import { lazyModule } from '../utils/lazyComponent';
import { showToast } from './store';
import { errorDetail } from '../utils/errorDetail';

const dispatcher = lazyModule(() => import('./actions/interactive-delivery'));

export function installDeferredInteractiveDelivery(): () => void {
  let stopped = false;
  let loading: Promise<void> | null = null;
  let stopDispatcher: (() => void) | undefined;
  let accept!: (service: InteractiveDeliveryService) => void;
  let cancel!: (error: Error) => void;
  const ready = new Promise<InteractiveDeliveryService>((resolve, reject) => { accept = resolve; cancel = reject; });
  // Teardown can happen before any caller has awaited initialization.
  void ready.catch(() => {});
  const failed = (error: unknown, retry: () => void) => {
    if (stopped) return;
    showToast(`Interactive request delivery failed: ${errorDetail(error)}`, 'error',
      { key: 'interactive-delivery-error', action: { label: 'Retry', onClick: retry } });
  };
  async function run<T>(action: (service: InteractiveDeliveryService) => Promise<T>): Promise<T> {
    const service = await ready;
    if (stopped) throw new Error('Interactive delivery stopped. Reopen Lucidos.');
    return action(service);
  }
  const uninstall = installInteractiveDeliveryService({
    formIds: () => run((service) => service.formIds()),
    synchronizeForms: (body) => run((service) => service.synchronizeForms(body)),
    reconcileForms: (known, open, resolved) => run((service) => service.reconcileForms(known, open, resolved)),
    submit: (request) => run((service) => service.submit(request)),
    resolve: (id) => run((service) => service.resolve(id)),
    publish: () => run((service) => service.publish()),
    reconsider: () => run((service) => service.reconsider()),
  }, failed);
  function retry(): void { void preload(); }
  function preload(): Promise<void> {
    if (stopped || stopDispatcher) return Promise.resolve();
    return loading ??= initialize().catch((error) => failed(error, retry)).finally(() => { loading = null; });
  }
  async function initialize(): Promise<void> {
    if (!await dispatcher.preload()) throw new Error('Delivery code could not load. Retry or refresh Lucidos.');
    if (stopped) return;
    stopDispatcher = dispatcher.get().installInteractiveDelivery((service) => {
      accept(service);
      return () => {};
    });
  }
  prefetchWhenIdle({ preload });
  return () => {
    stopped = true;
    uninstall();
    cancel(new Error('Interactive delivery stopped. Reopen Lucidos.'));
    stopDispatcher?.();
  };
}
