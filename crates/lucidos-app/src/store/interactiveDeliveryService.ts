import type { InteractiveRequest } from './interactiveDeliveryRecords';

export interface InteractiveDeliveryService {
  formIds(): Promise<string[]>;
  synchronizeForms(body: () => Promise<void>): Promise<void>;
  reconcileForms(known: string[], open: string[], resolved: string[]): Promise<void>;
  submit(request: InteractiveRequest): Promise<void>;
  resolve(id: string): Promise<void>;
  publish(): Promise<void>;
  reconsider(): Promise<void>;
}
let service: InteractiveDeliveryService | null = null;
let failed: (error: unknown, retry: () => void) => void = (error) => { throw error; };

export function installInteractiveDeliveryService(
  next: InteractiveDeliveryService,
  onFailure: typeof failed,
): () => void {
  service = next;
  failed = onFailure;
  return () => { if (service === next) service = null; };
}

function perform(action: (current: InteractiveDeliveryService) => Promise<void>): void {
  const attempt = () => {
    if (!service) {
      failed(new Error('Interactive delivery is not ready. Reopen Lucidos and retry.'), attempt);
      return;
    }
    void action(service).catch((error) => {
      failed(error, attempt);
      void service?.reconsider().catch(() => {});
    });
  };
  attempt();
}

export function submitInteractiveRequest(request: Omit<InteractiveRequest, 'id'>, id: string | undefined): void {
  if (!id) {
    failed(new Error('The interactive request has no event identity. Reconnect Lucidos and retry from its thread.'), () => location.reload());
    return;
  }
  perform((current) => current.submit({ ...request, id }));
}
export function resolveInteractiveForm(id: string): void {
  perform((current) => current.resolve(`form:${id}`));
}

export async function reconcileInteractiveForms(known: string[], open: string[], resolved: string[]): Promise<void> {
  if (!known.length) return;
  const keys = (ids: string[]) => ids.map((id) => `form:${id}`);
  if (!service) throw new Error('Interactive delivery is not ready. Reopen Lucidos and retry.');
  const current = service;
  try { await current.reconcileForms(keys(known), keys(open), keys(resolved)); }
  catch (error) {
    await current.reconsider().catch(() => {});
    throw error;
  }
}

export async function snapshotInteractiveForms(): Promise<string[]> {
  if (!service) throw new Error('Interactive delivery is not ready. Reopen Lucidos and retry.');
  return service.formIds();
}

export async function synchronizeInteractiveForms(body: () => Promise<void>): Promise<void> {
  if (!service) throw new Error('Interactive delivery is not ready. Reopen Lucidos and retry.');
  await service.synchronizeForms(body);
}
