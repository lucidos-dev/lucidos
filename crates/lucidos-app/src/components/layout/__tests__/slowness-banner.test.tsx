/**
 * The slowness bar, component side: which instance renders and the bar's
 * markup. The dismissal and per-workspace rules are tested in
 * `store/actions/slowness.test.ts` and the wording in
 * `utils/slownessNotice.test.ts`.
 */
import { describe, expect, it, vi } from 'vitest';
import type { SlownessEpisode } from '../../../store/actions/slowness';
import { slownessBannerBody, shouldRenderSlownessBanner } from '../SlownessBanner';
import { findByClass, textOf } from './vnodeWalk';

const episode: SlownessEpisode = {
  state: 'slow',
  episode_id: 'e1',
  reason: 'memory',
  top_users: [
    { name: 'Google Chrome', bytes: 7e9, kind: 'app' },
    { name: 'Lucidos', bytes: 1.4e9, kind: 'lucidos' },
  ],
};

const unclear: SlownessEpisode = {
  state: 'slow',
  episode_id: 'e2',
  reason: 'unclear',
  busiest_apps: [
    { name: 'Xcode', percent: 40, kind: 'app' },
    { name: 'Lucidos', percent: 12, kind: 'lucidos' },
  ],
  slow_workspaces: ['dev'],
};

describe('shouldRenderSlownessBanner renders exactly one instance per layout', () => {
  it('renders only the desktop instance on a desktop viewport', () => {
    const args = { mobileViewport: false, episode };
    expect(shouldRenderSlownessBanner({ layout: 'desktop', ...args })).toBe(true);
    expect(shouldRenderSlownessBanner({ layout: 'mobile', ...args })).toBe(false);
  });

  it('renders only the mobile instance on a mobile viewport', () => {
    const args = { mobileViewport: true, episode };
    expect(shouldRenderSlownessBanner({ layout: 'mobile', ...args })).toBe(true);
    expect(shouldRenderSlownessBanner({ layout: 'desktop', ...args })).toBe(false);
  });

  it('renders neither without an episode', () => {
    for (const mobileViewport of [true, false]) {
      for (const layout of ['desktop', 'mobile'] as const) {
        expect(shouldRenderSlownessBanner({ layout, mobileViewport, episode: null })).toBe(false);
      }
    }
  });
});

describe('slownessBannerBody renders the bar', () => {
  const body = (onDismiss = () => {}) =>
    slownessBannerBody({ layout: 'desktop', episode, onDismiss });

  it('says what is wrong, who holds the memory, and what to do', () => {
    const text = textOf(body());
    expect(text).toContain('The computer running Lucidos is short on memory, so Lucidos is slow.');
    expect(text).toContain('Biggest users: Google Chrome 7.0 GB, Lucidos 1.4 GB.');
    expect(text).toContain('Quit or restart Google Chrome to free memory.');
  });

  it('omits the list when nothing was measured yet', () => {
    const text = textOf(slownessBannerBody({
      layout: 'desktop',
      episode: { ...episode, top_users: [] },
      onDismiss: () => {},
    }));
    expect(text).not.toContain('Biggest users');
    expect(text).toContain('Quit apps you are not using to free memory.');
  });

  it('says only that Lucidos is slow when memory is not the cause, and names the busiest apps', () => {
    const text = textOf(slownessBannerBody({ layout: 'desktop', episode: unclear, onDismiss: () => {} }));
    expect(text).toContain('Lucidos is responding slowly.');
    expect(text).toContain('Busiest apps: Xcode 40%, Lucidos 12%.');
    expect(text).toContain('Quit or restart Xcode.');
    expect(text).not.toMatch(/memory/i);
  });

  it('never lists bystanders when nothing is busy, and gives the general tip', () => {
    const text = textOf(slownessBannerBody({
      layout: 'desktop',
      episode: { ...unclear, busiest_apps: [{ name: 'Google Chrome', percent: 3, kind: 'app' }] },
      onDismiss: () => {},
    }));
    expect(text).not.toContain('Busiest apps');
    expect(text).toContain('Nothing on this computer stands out as busy.');
  });

  it('dismisses through its close button', () => {
    const onDismiss = vi.fn();
    const [close] = findByClass(body(onDismiss), 'slowness-banner-close');
    (close.props.onClick as () => void)();
    expect(onDismiss).toHaveBeenCalledOnce();
  });

  it('carries no left-accent stripe hook', () => {
    const classes = findByClass(body(), 'slowness-banner')[0].props.class as string;
    expect(classes).toBe('slowness-banner');
  });
});
