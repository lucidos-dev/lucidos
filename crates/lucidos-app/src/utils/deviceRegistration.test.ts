/**
 * A mutation waits for this page's device registration, and nothing else does.
 *
 * The e2e flake that found this: a first keystroke started a compose while
 * registration was still in flight. The engine refused it with a 401, and the
 * rollback cleared the reader's draft.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

type Module = typeof import('./deviceRegistration');

async function freshModule(): Promise<Module> {
  vi.resetModules();
  return import('./deviceRegistration');
}

function deferred(): { promise: Promise<void>; resolve: () => void } {
  let resolve!: () => void;
  const promise = new Promise<void>((r) => { resolve = r; });
  return { promise, resolve };
}

describe('registrationToAwait', () => {
  let mod: Module;
  beforeEach(async () => { mod = await freshModule(); });
  afterEach(() => { vi.useRealTimers(); });

  it('has nothing to wait for before any registration starts', () => {
    expect(mod.registrationToAwait('/api/v1/threads', 'POST')).toBeNull();
  });

  it('holds a mutation until a pending registration settles', async () => {
    const attempt = deferred();
    mod.trackDeviceRegistration(attempt.promise);
    const wait = mod.registrationToAwait('/api/v1/threads', 'POST');
    expect(wait).not.toBeNull();

    let released = false;
    void wait!.then(() => { released = true; });
    await Promise.resolve();
    expect(released).toBe(false);

    attempt.resolve();
    await wait;
    expect(released).toBe(true);
  });

  it('lets a read through while registration is pending', () => {
    mod.trackDeviceRegistration(deferred().promise);
    expect(mod.registrationToAwait('/api/v1/threads/list')).toBeNull();
    expect(mod.registrationToAwait('/api/v1/threads/list', 'get')).toBeNull();
  });

  it('never makes the registration wait on itself', () => {
    mod.trackDeviceRegistration(deferred().promise);
    expect(mod.registrationToAwait('/api/v1/devices/register', 'POST')).toBeNull();
  });

  it('has nothing to wait for once registration has settled', async () => {
    await mod.trackDeviceRegistration(Promise.resolve());
    expect(mod.registrationToAwait('/api/v1/threads', 'POST')).toBeNull();
  });

  it('gives up on a registration that hangs, after three seconds', async () => {
    vi.useFakeTimers();
    mod.trackDeviceRegistration(new Promise<void>(() => {}));
    const wait = mod.registrationToAwait('/api/v1/threads', 'POST')!;
    let released = false;
    void wait.then(() => { released = true; });

    await vi.advanceTimersByTimeAsync(2999);
    expect(released).toBe(false);
    await vi.advanceTimersByTimeAsync(1);
    expect(released).toBe(true);
  });
});
