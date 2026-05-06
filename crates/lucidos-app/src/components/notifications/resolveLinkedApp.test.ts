import { describe, it, expect } from 'vitest';
import { resolveLinkedApp } from './resolveLinkedApp';
import type { App, Loadable } from '../../store/types';

const appList: App[] = [
  { id: 'morning-dashboard', name: 'Dashboard', description: '', knowhow: [] },
  { id: 'habit-tracker', name: 'Habit Tracker', description: '', knowhow: [] },
];
const loaded: Loadable<App[]> = { status: 'loaded', data: appList };

describe('resolveLinkedApp', () => {
  it('returns linked when app_id resolves', () => {
    expect(resolveLinkedApp('morning-dashboard', loaded)).toEqual({
      kind: 'linked',
      app: appList[0],
    });
  });

  it('returns unknown when app_id is set but no app matches', () => {
    expect(resolveLinkedApp('does-not-exist', loaded)).toEqual({
      kind: 'unknown',
      appId: 'does-not-exist',
    });
  });

  it('returns none when app_id is undefined', () => {
    expect(resolveLinkedApp(undefined, loaded)).toEqual({ kind: 'none' });
  });

  it('returns none when app_id is null', () => {
    expect(resolveLinkedApp(null, loaded)).toEqual({ kind: 'none' });
  });

  it('returns none when app_id is the empty string', () => {
    expect(resolveLinkedApp('', loaded)).toEqual({ kind: 'none' });
  });

  it('does not match by title — title-match fallback is gone', () => {
    // A notification whose title equals an app name but with no app_id must NOT auto-link.
    // Previously this would fall back to apps.find(a => a.name === title); that path is removed.
    expect(resolveLinkedApp(undefined, loaded)).toEqual({ kind: 'none' });
  });

  it('returns pending when apps are still loading', () => {
    // Cold-start deep-link / push can open the modal before loadApps() resolves;
    // suppressing the 'unknown' verdict here prevents a false error flash.
    expect(resolveLinkedApp('morning-dashboard', { status: 'loading' })).toEqual({ kind: 'pending' });
  });

  it('returns pending when apps have not been fetched yet', () => {
    expect(resolveLinkedApp('morning-dashboard', { status: 'not-loaded' })).toEqual({ kind: 'pending' });
  });

  it('returns pending when apps failed to load', () => {
    // We can't tell stale-id from we-don't-know — withhold the unknown verdict.
    expect(resolveLinkedApp('morning-dashboard', { status: 'failed', error: 'oops' })).toEqual({ kind: 'pending' });
  });

  it('still returns none when app_id is absent, regardless of apps state', () => {
    expect(resolveLinkedApp(undefined, { status: 'loading' })).toEqual({ kind: 'none' });
    expect(resolveLinkedApp(null, { status: 'failed', error: 'oops' })).toEqual({ kind: 'none' });
  });
});
