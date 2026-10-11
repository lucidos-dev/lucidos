import { describe, it, expect } from 'vitest';
import { resolveLinkedApp } from './resolveLinkedApp';
import type { App, Loadable } from '../../store/types';

const appList: App[] = [
  { id: 'morning-dashboard', name: 'Dashboard', description: '', reveal: 'on-load', kind: 'app', reusable: false },
  { id: 'habit-tracker', name: 'Habit Tracker', description: '', reveal: 'on-load', kind: 'app', reusable: false },
];
const loaded: Loadable<App[]> = { status: 'loaded', data: appList };
const unread: Loadable<App | null> = { status: 'not-loaded' };
const fareGrid: App = { id: 'fare-grid', name: 'Fare grid', description: '', reveal: 'on-load', kind: 'widget', reusable: false };

describe('resolveLinkedApp', () => {
  it('returns linked when app_id resolves', () => {
    expect(resolveLinkedApp('morning-dashboard', loaded, unread)).toEqual({
      kind: 'linked',
      app: appList[0],
    });
  });

  it('returns unknown once the by-id read says the app is gone', () => {
    expect(resolveLinkedApp('does-not-exist', loaded, { status: 'loaded', data: null })).toEqual({
      kind: 'unknown',
      appId: 'does-not-exist',
    });
  });

  it('returns none when app_id is undefined', () => {
    expect(resolveLinkedApp(undefined, loaded, unread)).toEqual({ kind: 'none' });
  });

  it('returns none when app_id is null', () => {
    expect(resolveLinkedApp(null, loaded, unread)).toEqual({ kind: 'none' });
  });

  it('returns none when app_id is the empty string', () => {
    expect(resolveLinkedApp('', loaded, unread)).toEqual({ kind: 'none' });
  });

  it('does not match by title — title-match fallback is gone', () => {
    // A notification whose title equals an app name but with no app_id must NOT auto-link.
    // Previously this would fall back to apps.find(a => a.name === title); that path is removed.
    expect(resolveLinkedApp(undefined, loaded, unread)).toEqual({ kind: 'none' });
  });

  it('returns pending when apps are still loading', () => {
    // Cold-start deep-link / push can open the modal before loadApps() resolves;
    // suppressing the 'unknown' verdict here prevents a false error flash.
    expect(resolveLinkedApp('morning-dashboard', { status: 'loading' }, unread)).toEqual({ kind: 'pending' });
  });

  it('returns pending when apps have not been fetched yet', () => {
    expect(resolveLinkedApp('morning-dashboard', { status: 'not-loaded' }, unread)).toEqual({ kind: 'pending' });
  });

  it('returns pending when apps failed to load', () => {
    // We can't tell stale-id from we-don't-know — withhold the unknown verdict.
    expect(resolveLinkedApp('morning-dashboard', { status: 'failed', error: 'oops' }, unread)).toEqual({ kind: 'pending' });
  });

  it('still returns none when app_id is absent, regardless of apps state', () => {
    expect(resolveLinkedApp(undefined, { status: 'loading' }, unread)).toEqual({ kind: 'none' });
    expect(resolveLinkedApp(null, { status: 'failed', error: 'oops' }, unread)).toEqual({ kind: 'none' });
  });

  it('links a widget, which the apps list never holds, once its read lands', () => {
    expect(resolveLinkedApp('fare-grid', loaded, { status: 'loaded', data: fareGrid }))
      .toEqual({ kind: 'linked', app: fareGrid });
  });

  it('waits on the read before calling an unlisted id unknown', () => {
    expect(resolveLinkedApp('fare-grid', loaded, unread)).toEqual({ kind: 'pending' });
    expect(resolveLinkedApp('fare-grid', loaded, { status: 'loading' })).toEqual({ kind: 'pending' });
    expect(resolveLinkedApp('fare-grid', loaded, { status: 'failed', error: 'offline' })).toEqual({ kind: 'pending' });
  });
});
