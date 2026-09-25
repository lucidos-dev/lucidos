import { afterEach, describe, expect, it, vi } from 'vitest';
import type { SlownessStatus } from '../../api/client/control';
import {
  dismissedSlownessEpisode,
  dismissSlownessEpisode,
  slownessStatus,
  refreshSlowness,
  visibleSlownessEpisode,
} from './slowness';

const low = (episode_id: string): SlownessStatus => ({
  state: 'slow',
  episode_id,
  reason: 'memory',
  top_users: [{ name: 'Google Chrome', bytes: 7e9, kind: 'app' }],
});

const unclear = (slow_workspaces: string[]): SlownessStatus => ({
  state: 'slow',
  episode_id: 'u',
  reason: 'unclear',
  busiest_apps: [{ name: 'Xcode', percent: 40, kind: 'app' }],
  slow_workspaces,
});

afterEach(async () => {
  // A success ends any failing streak, so no test inherits one.
  vi.stubGlobal('fetch', vi.fn(async () => new Response(JSON.stringify({ state: 'normal' }))));
  await refreshSlowness();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  localStorage.clear();
  dismissedSlownessEpisode.value = null;
  slownessStatus.value = null;
});

describe('visibleSlownessEpisode', () => {
  it('shows a memory episode in every workspace', () => {
    expect(visibleSlownessEpisode(low('a'), null, 'dev')).toEqual(low('a'));
    expect(visibleSlownessEpisode(low('a'), null, 'notes')).toEqual(low('a'));
  });

  it('shows an unclear episode only in a workspace that was slow', () => {
    expect(visibleSlownessEpisode(unclear(['dev']), null, 'dev')).toEqual(unclear(['dev']));
    expect(visibleSlownessEpisode(unclear(['dev']), null, 'notes')).toBeNull();
    expect(visibleSlownessEpisode(unclear(['dev']), null, null)).toBeNull();
  });

  it('shows nothing for a normal machine, an unknown answer, or an unknown reason', () => {
    expect(visibleSlownessEpisode({ state: 'normal' }, null, 'dev')).toBeNull();
    expect(visibleSlownessEpisode(null, null, 'dev')).toBeNull();
    const future = { state: 'slow', episode_id: 'f', reason: 'disk' } as unknown as SlownessStatus;
    expect(visibleSlownessEpisode(future, null, 'dev')).toBeNull();
  });

  it('hides the dismissed episode and shows the next one', () => {
    dismissSlownessEpisode('a');
    expect(visibleSlownessEpisode(low('a'), dismissedSlownessEpisode.value, 'dev')).toBeNull();
    expect(visibleSlownessEpisode(low('b'), dismissedSlownessEpisode.value, 'dev')).toEqual(low('b'));
  });

  it('remembers the dismissal on this device', () => {
    dismissSlownessEpisode('a');
    expect(localStorage.getItem('slowness-dismissed-episode')).toBe('a');
  });
});

describe('refreshSlowness', () => {
  it('records the gateway answer', async () => {
    vi.stubGlobal('fetch', vi.fn(async () => new Response(JSON.stringify(low('a')))));
    await refreshSlowness();
    expect(slownessStatus.value).toEqual(low('a'));
  });

  it('drops a stale episode when the gateway cannot answer', async () => {
    slownessStatus.value = low('a');
    vi.spyOn(console, 'warn').mockImplementation(() => {});
    vi.stubGlobal('fetch', vi.fn(async () => new Response('gone', { status: 404 })));
    await refreshSlowness();
    expect(slownessStatus.value).toBeNull();
  });

  it('warns once per failing streak, not once per poll', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    const fail = vi.fn(async () => new Response('gone', { status: 404 }));
    vi.stubGlobal('fetch', fail);
    await refreshSlowness();
    await refreshSlowness();
    expect(warn).toHaveBeenCalledTimes(1);

    vi.stubGlobal('fetch', vi.fn(async () => new Response(JSON.stringify(low('a')))));
    await refreshSlowness();
    vi.stubGlobal('fetch', fail);
    await refreshSlowness();
    expect(warn).toHaveBeenCalledTimes(2);
  });
});
