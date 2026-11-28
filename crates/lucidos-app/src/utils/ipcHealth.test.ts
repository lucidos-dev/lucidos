import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import {
  onIpcFailure,
  onIpcSuccess,
  describeIpcError,
  healthyIpc,
  REPORT_INTERVAL_MS,
  type IpcHealth,
} from './ipcHealth';

describe('IPC health reporting', () => {
  it('reports the very first failure immediately', () => {
    const { health, report } = onIpcFailure(healthyIpc, 1_000);
    expect(report).toBe('failing');
    expect(health).toEqual({ failures: 1, reportedAt: 1_000 });
  });

  it('rate-limits a permanently broken bridge instead of drowning the log', () => {
    let health: IpcHealth = healthyIpc;
    let reported = 0;
    for (let t = 0; t < 60 * 60 * 1000; t += 15_000) {
      const outcome = onIpcFailure(health, t);
      health = outcome.health;
      if (outcome.report === 'failing') reported += 1;
    }
    expect(health.failures).toBe(240);
    expect(reported).toBe(12);
  });

  it('reports again once the rate-limit window has passed', () => {
    const first = onIpcFailure(healthyIpc, 0);
    expect(first.report).toBe('failing');
    const inside = onIpcFailure(first.health, REPORT_INTERVAL_MS - 1);
    expect(inside.report).toBeNull();
    expect(inside.health.reportedAt).toBe(0);
    const due = onIpcFailure(inside.health, REPORT_INTERVAL_MS);
    expect(due.report).toBe('failing');
    expect(due.health.reportedAt).toBe(REPORT_INTERVAL_MS);
    expect(due.health.failures).toBe(3);
  });

  it('closes out a reported outage with exactly one recovery line', () => {
    const failed = onIpcFailure(healthyIpc, 0);
    const recovered = onIpcSuccess(failed.health);
    expect(recovered.report).toBe('recovered');
    expect(recovered.health).toEqual(healthyIpc);
    expect(onIpcSuccess(recovered.health).report).toBeNull();
  });

  it('stays silent when nothing was ever reported', () => {
    expect(onIpcSuccess(healthyIpc).report).toBeNull();
  });

  it('re-arms after recovery so a second outage is reported at once', () => {
    const first = onIpcFailure(healthyIpc, 0);
    const recovered = onIpcSuccess(first.health);
    const second = onIpcFailure(recovered.health, 10);
    expect(second.report).toBe('failing');
    expect(second.health.failures).toBe(1);
  });

  it('trims error text so the engine does not reject the breadcrumb', () => {
    expect(describeIpcError(new Error('boom'))).toBe('boom');
    expect(describeIpcError('Command heartbeat not allowed by ACL')).toBe(
      'Command heartbeat not allowed by ACL',
    );
    expect(describeIpcError(undefined)).toBe('undefined');
    expect(describeIpcError(new Error('x'.repeat(5_000))).length).toBe(200);
  });
});

describe('invoke() feeds the IPC health channel', () => {
  const posted: Array<{ category: string; message: string; data: Record<string, unknown> }> = [];

  beforeEach(() => {
    posted.length = 0;
    vi.resetModules();
    vi.doMock('./clientLog', () => ({
      postClientLog: (category: string, message: string, data: Record<string, unknown>) => {
        posted.push({ category, message, data });
      },
    }));
  });

  afterEach(() => {
    vi.doUnmock('./clientLog');
    vi.resetModules();
    delete (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
  });

  it('logs a rejected command and leaves the rejection intact for the caller', async () => {
    (window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {
      invoke: () => Promise.reject(new Error('Command heartbeat not allowed by ACL')),
      transformCallback: () => 0,
    };
    const { invoke } = await import('./tauri');

    await expect(invoke('heartbeat')).rejects.toThrow('not allowed by ACL');

    expect(posted).toHaveLength(1);
    expect(posted[0].category).toBe('ipc');
    expect(posted[0].message).toBe('invoke-failed');
    expect(posted[0].data).toMatchObject({
      command: 'heartbeat',
      failures: 1,
      error: 'Command heartbeat not allowed by ACL',
    });
  });

  it('says nothing while the bridge works', async () => {
    (window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {
      invoke: () => Promise.resolve('ok'),
      transformCallback: () => 0,
    };
    const { invoke } = await import('./tauri');

    await expect(invoke('heartbeat')).resolves.toBe('ok');
    await expect(invoke('heartbeat')).resolves.toBe('ok');
    expect(posted).toHaveLength(0);
  });

  it('does not treat another command succeeding as recovery', async () => {
    (window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {
      invoke: (cmd: string) =>
        cmd === 'show_native_notification'
          ? Promise.reject(new Error('Command show_native_notification not allowed by ACL'))
          : Promise.resolve('ok'),
      transformCallback: () => 0,
    };
    const { invoke } = await import('./tauri');

    for (let i = 0; i < 4; i++) {
      await expect(invoke('show_native_notification')).rejects.toThrow('not allowed by ACL');
      await expect(invoke('heartbeat')).resolves.toBe('ok');
    }

    expect(posted).toHaveLength(1);
    expect(posted[0].message).toBe('invoke-failed');
    expect(posted[0].data).toMatchObject({ command: 'show_native_notification', failures: 1 });
  });

  it('tracks each failing command separately', async () => {
    (window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {
      invoke: (cmd: string) => Promise.reject(new Error(`${cmd} denied`)),
      transformCallback: () => 0,
    };
    const { invoke } = await import('./tauri');

    await expect(invoke('heartbeat')).rejects.toThrow();
    await expect(invoke('nudge_dock_badge')).rejects.toThrow();
    await expect(invoke('heartbeat')).rejects.toThrow();

    expect(posted.map((p) => p.data.command)).toEqual(['heartbeat', 'nudge_dock_badge']);
  });

  it('logs the recovery when the bridge comes back', async () => {
    let fail = true;
    (window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {
      invoke: () => (fail ? Promise.reject(new Error('denied')) : Promise.resolve('ok')),
      transformCallback: () => 0,
    };
    const { invoke } = await import('./tauri');

    await expect(invoke('heartbeat')).rejects.toThrow('denied');
    fail = false;
    await expect(invoke('heartbeat')).resolves.toBe('ok');

    expect(posted.map((p) => p.message)).toEqual(['invoke-failed', 'invoke-recovered']);
    expect(posted[1].data).toMatchObject({ command: 'heartbeat', after_failures: 1 });
  });
});
