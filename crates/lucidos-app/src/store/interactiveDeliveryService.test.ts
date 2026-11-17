import { expect, it, vi } from 'vitest';
import { installInteractiveDeliveryService, reconcileInteractiveForms, type InteractiveDeliveryService } from './interactiveDeliveryService';

it('recovers a failed reconciliation claim before reporting the original error', async () => {
  const failure = new Error('Receipt write failed');
  let finishRecovery!: () => void;
  const recovery = new Promise<void>((resolve) => { finishRecovery = resolve; });
  const service: InteractiveDeliveryService = {
    formIds: vi.fn(async () => []),
    synchronizeForms: (body) => body(),
    reconcileForms: vi.fn().mockRejectedValue(failure),
    submit: vi.fn(), resolve: vi.fn(), publish: vi.fn(),
    reconsider: vi.fn(() => recovery),
  };
  const uninstall = installInteractiveDeliveryService(service, vi.fn());
  try {
    let settled = false;
    const result = reconcileInteractiveForms(['request'], [], []).catch((error) => {
      settled = true;
      return error;
    });
    await vi.waitFor(() => expect(service.reconsider).toHaveBeenCalledOnce());
    expect(settled).toBe(false);
    finishRecovery();
    expect(await result).toBe(failure);
    expect(service.reconcileForms).toHaveBeenCalledWith(['form:request'], [], []);

    vi.mocked(service.reconsider).mockRejectedValueOnce(new Error('Storage unavailable'));
    await expect(reconcileInteractiveForms(['request'], [], [])).rejects.toBe(failure);
  } finally { finishRecovery(); uninstall(); }
});
