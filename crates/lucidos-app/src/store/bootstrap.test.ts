// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, readdirSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, join, relative, sep } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { BootstrapRead, BootstrapSliceKey, BootstrapSnapshot } from '../api/client/bootstrap';

const fetchBootstrap = vi.hoisted(() => vi.fn<(etag: string | null) => Promise<BootstrapRead>>());
vi.mock('../api/client/bootstrap', () => ({ fetchBootstrap }));

import { BOOTSTRAP_SLICES, requestBootstrap } from './bootstrap';
import { triggers, webhookIngress } from './store';
import { devices } from './actions/devices';
import { users } from './actions/users';

const EVERY_SLICE: Record<BootstrapSliceKey, true> = {
  preferences: true, unread_notifications: true, reader_devices: true, users: true, pinned_apps: true,
  apps: true, chat_models: true, intelligence_ladder: true, spend_cap_holds: true,
  last_successful_backup: true, release_notices: true, embedding_model_status: true,
  webhook_ingress: true, webhook_refusals: true, installed_plugins: true, plugin_catalog: true,
  triggers: true, historical_triggers: true, trigger_groups: true, thread_queue: true,
  repositories: true, filter_facets: true, devices: true, changes_state: true,
  engine_version_status: true,
};

function snapshotWith(slices: Partial<BootstrapSnapshot>): BootstrapSnapshot {
  const failed = Object.fromEntries(Object.keys(EVERY_SLICE).map(
    (key) => [key, { error: { message: `${key} is down`, http_code: 503 } }],
  ));
  return { ...failed, ...slices } as BootstrapSnapshot;
}

beforeEach(() => {
  fetchBootstrap.mockReset();
  users.value = { status: 'not-loaded' };
  devices.value = { status: 'not-loaded' };
  triggers.value = { status: 'not-loaded' };
  webhookIngress.value = { status: 'not-loaded' };
});

describe('the slice table', () => {
  it('has a row with an apply, a fail and a loader for every slice of the snapshot', () => {
    expect(Object.keys(BOOTSTRAP_SLICES).sort()).toEqual(Object.keys(EVERY_SLICE).sort());
    for (const entry of Object.values(BOOTSTRAP_SLICES)) {
      expect(typeof entry.slice.begin).toBe('function');
      expect(typeof entry.slice.apply).toBe('function');
      expect(typeof entry.slice.fail).toBe('function');
      expect(typeof entry.load).toBe('function');
    }
  });

  it('applies preferences first, because the engine version slice waits on them', () => {
    expect(Object.keys(BOOTSTRAP_SLICES)[0]).toBe('preferences');
  });

  it('names only an earlier row as the slice a loader runs after', () => {
    const keys = Object.keys(BOOTSTRAP_SLICES);
    for (const [index, entry] of Object.values(BOOTSTRAP_SLICES).entries()) {
      if (entry.after !== undefined) expect(keys.slice(0, index)).toContain(entry.after);
    }
  });
});

describe('a snapshot applied to the store', () => {
  it('loads a data slice and fails an error slice with its own HTTP code', async () => {
    fetchBootstrap.mockResolvedValue({
      kind: 'snapshot',
      etag: 'W/"1"',
      snapshot: snapshotWith({
        users: { data: { users: [], you: null, team: false } },
        triggers: { data: { triggers: [] } },
        devices: { error: { message: 'devices table is locked', http_code: 500 } },
      }),
    });

    await requestBootstrap();

    expect(users.value).toEqual({ status: 'loaded', data: { users: [], you: null, team: false } });
    expect(triggers.value).toEqual({ status: 'loaded', data: [] });
    expect(devices.value).toEqual({ status: 'failed', error: 'devices table is locked', httpCode: 500 });
  });

  it('keeps what a slice already showed when its re-read fails, where its loader does', async () => {
    const standing = { status: 'loaded' as const, data: { ingress: { outage: null }, receivedAt: 1 } };
    webhookIngress.value = standing as unknown as typeof webhookIngress.value;
    fetchBootstrap.mockResolvedValue({ kind: 'snapshot', etag: null, snapshot: snapshotWith({}) });

    await requestBootstrap();

    expect(webhookIngress.value).toBe(standing);
    expect(users.value).toEqual({ status: 'failed', error: 'users is down', httpCode: 503 });
  });
});

describe('Phase 2 changes no behaviour', () => {
  const src = join(dirname(fileURLToPath(import.meta.url)), '..');

  function sources(dir: string): string[] {
    return readdirSync(dir, { withFileTypes: true }).flatMap((entry: { name: string; isDirectory(): boolean }) => {
      const path = join(dir, entry.name);
      if (entry.isDirectory()) return entry.name === 'generated' ? [] : sources(path);
      return /\.tsx?$/.test(entry.name) ? [path] : [];
    });
  }

  it('lets no module call the bootstrap until the engine serves the route', () => {
    const own = /^store\/bootstrap(Requester)?(\.test)?\.ts$/;
    const callers = sources(src)
      .map((path) => relative(src, path).split(sep).join('/'))
      .filter((path) => !own.test(path))
      .filter((path) => /\b(requestBootstrap|holdGlobalFrame|fetchBootstrap)\b/.test(readFileSync(join(src, path), 'utf8')))
      .sort();
    expect(callers).toEqual(['api/client/bootstrap.test.ts', 'api/client/bootstrap.ts']);
  });
});
