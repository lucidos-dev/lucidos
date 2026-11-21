import { describe, it, expect } from 'vitest';
import { backupSlotsPending } from '../BackupSection';
import type { BackupProviderInfo, BackupStatus } from '../../../api/client';
import type { Loadable } from '../../../store/types';

const READY: Loadable<BackupProviderInfo[]> = {
  status: 'loaded',
  data: [{ id: 'google-drive', name: 'Google Drive', connected: true, ready: true, missing_scopes: [], folder_url: null }],
};

const STATUS: BackupStatus = {
  running: false,
  last_run: null,
  latest_backup: null,
  age_seconds: null,
  stale: false,
  list_error: null,
};

describe('backupSlotsPending', () => {
  it('holds every slot from the first render, before any read has started', () => {
    const p = backupSlotsPending({ status: 'not-loaded' }, false, { status: 'not-loaded' });
    expect(p).toEqual({ controls: true, status: true });
  });

  it('holds the health card while the registry is in flight', () => {
    // The status read is per destination, so it cannot be issued until the
    // registry lands. Keyed on the status read alone, the card sat at zero
    // height for that whole read. It then grew a box at the top of a page that
    // already looked settled.
    const p = backupSlotsPending({ status: 'loading' }, false, { status: 'not-loaded' });
    expect(p.status).toBe(true);
  });

  it('keeps holding the health card in the frame between the registry and its own read', () => {
    // The status fetch is started by an effect, which runs AFTER the render that
    // settled the registry. A gate reading `status === "loading"` went false for
    // that one frame, and `useDelayedFlag` resets on any drop: the skeleton
    // blinked out and came back 300ms later as a second wave.
    const p = backupSlotsPending(READY, true, { status: 'not-loaded' });
    expect(p).toEqual({ controls: false, status: true });
  });

  it('holds the health card while its own read is in flight', () => {
    expect(backupSlotsPending(READY, true, { status: 'loading' }).status).toBe(true);
  });

  it('releases the health card once the status is known', () => {
    expect(backupSlotsPending(READY, true, { status: 'loaded', data: STATUS }).status).toBe(false);
  });

  it('releases the health card on a failed status: the card draws that itself', () => {
    expect(backupSlotsPending(READY, true, { status: 'failed', error: 'boom' }).status).toBe(false);
  });

  it('releases the health card when no destination is ready, since no card is coming', () => {
    expect(backupSlotsPending(READY, false, { status: 'not-loaded' })).toEqual({ controls: false, status: false });
  });

  it('releases every slot when the registry read failed', () => {
    const p = backupSlotsPending({ status: 'failed', error: 'boom' }, false, { status: 'not-loaded' });
    expect(p).toEqual({ controls: false, status: false });
  });

  it('stays pending without a break across a whole cold open', () => {
    // What makes the section shimmer ONCE rather than in waves: the shared delay
    // gate is armed on the first render and stays armed until the last first-load
    // read settles. One false anywhere in this sequence disarms it, and the next
    // slot to appear then starts its own 300ms wave.
    const coldOpen: [Loadable<BackupProviderInfo[]>, boolean, Loadable<BackupStatus>][] = [
      [{ status: 'not-loaded' }, false, { status: 'not-loaded' }],
      [{ status: 'loading' }, false, { status: 'not-loaded' }],
      [READY, true, { status: 'not-loaded' }],
      [READY, true, { status: 'loading' }],
    ];
    for (const [providers, ready, status] of coldOpen) {
      const p = backupSlotsPending(providers, ready, status);
      expect(p.controls || p.status).toBe(true);
    }
    const settled = backupSlotsPending(READY, true, { status: 'loaded', data: STATUS });
    expect(settled.controls || settled.status).toBe(false);
  });
});
