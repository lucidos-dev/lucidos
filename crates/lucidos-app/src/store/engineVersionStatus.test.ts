import { describe, expect, it, vi } from 'vitest';
import { signal } from '@preact/signals';
import { engineVersionStatusVersion, subscribeEngineVersionStatus } from './engineVersionStatus';

describe('version status readiness', () => {
  it('refreshes when a later preferences load settles and disposes the subscription', () => {
    const ready = signal(false);
    const refresh = vi.fn();
    const stop = subscribeEngineVersionStatus(refresh, () => ready.value);
    engineVersionStatusVersion.value++;
    expect(refresh).not.toHaveBeenCalled();
    // The startup request can remain hung; any successful load settles this signal.
    ready.value = true;
    expect(refresh).toHaveBeenCalledTimes(1);
    engineVersionStatusVersion.value++;
    expect(refresh).toHaveBeenCalledTimes(2);
    stop();
    engineVersionStatusVersion.value++;
    expect(refresh).toHaveBeenCalledTimes(2);
  });

  it('leaves the initial read to background checks when already settled', () => {
    const refresh = vi.fn();
    const stop = subscribeEngineVersionStatus(refresh, () => true);
    expect(refresh).not.toHaveBeenCalled();
    stop();
  });

  it('refreshes when a preferences reload changes the switch dismissal', () => {
    const dismissal = signal<string | undefined>(undefined);
    const refresh = vi.fn();
    const stop = subscribeEngineVersionStatus(refresh, () => true, () => dismissal.value);
    // A gap's status read can land before the preferences reload that carries
    // a peer's dismissal, so the reload itself must re-derive the toast.
    dismissal.value = 'build-a';
    expect(refresh).toHaveBeenCalledTimes(1);
    dismissal.value = 'build-a';
    expect(refresh).toHaveBeenCalledTimes(1);
    stop();
  });
});
