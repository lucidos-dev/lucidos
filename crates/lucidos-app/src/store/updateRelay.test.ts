import { describe, it, expect, beforeEach } from 'vitest';
import {
  RELAY_GIVE_UP_MS,
  loadRelayedUpdate,
  relayAvailable,
  saveRelayedUpdate,
  settleRelay,
} from './updateRelay';
import { updateRelay, releaseCheck, type RelayedUpdate } from './store';
import type { UpdateRelayRequest, GatewayStatus, ReleaseCheck } from '../api/client/control';

const MARKER: RelayedUpdate = { id: 'r-1', version: '1.3.0', fromVersion: '1.2.0', since: 1_000 };

function check(current: string, latest: string | null = '1.3.0'): ReleaseCheck {
  return {
    enabled: true,
    supported: true,
    current_version: current,
    checked_at: null,
    last_error: null,
    latest: latest ? { version: latest, notes: null, install: 'desktop-app', command: null } : null,
  };
}

function status(current: string, relay: UpdateRelayRequest | null): GatewayStatus {
  return {
    build_id: 'b',
    update_available: false,
    release_check: check(current),
    update_relay: { client: { version: current, blocker: null }, request: relay },
  };
}

function relay(state: UpdateRelayRequest['state'], progress: UpdateRelayRequest['progress'], id = 'r-1'): UpdateRelayRequest {
  return { id, version: '1.3.0', state, progress };
}

describe('settleRelay', () => {
  // The success the user waits for: the service came back on the new version.
  // The relay's own state died with it, so the version is the only proof.
  it('calls a newer running version a success, whatever the relay says', () => {
    expect(settleRelay(MARKER, status('1.3.0', null), 2_000)).toEqual({ kind: 'succeeded', version: '1.3.0' });
  });

  it('passes a running frame through', () => {
    const frame = { version: '1.3.0', phase: 'downloading' as const, downloaded: 5, total: 10 };
    expect(settleRelay(MARKER, status('1.2.0', relay('running', frame)), 2_000)).toEqual({ kind: 'progress', frame });
  });

  // A bundle-swap failure carries the recovery message, so the frame survives.
  it('passes an ending frame through', () => {
    const frame = { version: '1.3.0', phase: 'bundle-swap-failed' as const, message: 'Reinstall Lucidos from the .dmg to recover.' };
    expect(settleRelay(MARKER, status('1.2.0', relay('ended', frame)), 2_000)).toEqual({ kind: 'ended', frame });
  });

  it('waits while the request is unclaimed or the run has said nothing yet', () => {
    expect(settleRelay(MARKER, status('1.2.0', relay('requested', null)), 2_000)).toEqual({ kind: 'waiting' });
    expect(settleRelay(MARKER, status('1.2.0', relay('running', null)), 2_000)).toEqual({ kind: 'waiting' });
  });

  // The gateway is down for the restart. Waiting is right, but not forever.
  it('waits through a gateway outage, then gives up', () => {
    expect(settleRelay(MARKER, null, MARKER.since + 1)).toEqual({ kind: 'waiting' });
    expect(settleRelay(MARKER, null, MARKER.since + RELAY_GIVE_UP_MS)).toEqual({ kind: 'did-not-run' });
  });

  it('gives up on a request that never moves', () => {
    expect(settleRelay(MARKER, status('1.2.0', relay('requested', null)), MARKER.since + RELAY_GIVE_UP_MS))
      .toEqual({ kind: 'did-not-run' });
  });

  // The client quit, never claimed it, or the gateway restarted for another
  // reason. Nothing changed, and saying "updated" would be a lie.
  it('reports a vanished request on the old version as not run', () => {
    expect(settleRelay(MARKER, status('1.2.0', null), 2_000)).toEqual({ kind: 'did-not-run' });
    expect(settleRelay(MARKER, status('1.2.0', relay('running', null, 'r-2')), 2_000)).toEqual({ kind: 'did-not-run' });
  });

  // Codex's case: another update moved the version, but not to the release
  // this request asked for. That proves nothing about this request.
  it('needs the requested release, not just any newer version', () => {
    expect(settleRelay(MARKER, status('1.2.5', null), 2_000)).toEqual({ kind: 'did-not-run' });
    expect(settleRelay(MARKER, status('1.4.0', null), 2_000)).toEqual({ kind: 'succeeded', version: '1.4.0' });
  });

  // The service restart failed after the swap: the old gateway keeps the last
  // frame while the relaunched client beats on. The bound still ends the wait.
  it('gives up on a run whose last frame never moves on', () => {
    const frame = { version: '1.3.0', phase: 'relaunching' as const };
    expect(settleRelay(MARKER, status('1.2.0', relay('running', frame)), MARKER.since + RELAY_GIVE_UP_MS))
      .toEqual({ kind: 'did-not-run' });
  });

  it('never calls an older or equal version a success', () => {
    for (const current of ['1.2.0', '1.1.9']) {
      expect(settleRelay(MARKER, status(current, null), 2_000).kind).toBe('did-not-run');
    }
  });
});

describe('relayAvailable', () => {
  beforeEach(() => {
    releaseCheck.value = check('1.2.0');
    updateRelay.value = { client: { version: '1.2.0', blocker: null }, request: null };
  });

  it('is available for an attached, unblocked client behind the newest release', () => {
    expect(relayAvailable()).toBe(true);
  });

  it('is not available without a client, with a blocker, or without a newer release', () => {
    updateRelay.value = null;
    expect(relayAvailable()).toBe(false);
    updateRelay.value = { client: { version: '1.2.0', blocker: 'needs a password' }, request: null };
    expect(relayAvailable()).toBe(false);
    updateRelay.value = { client: { version: '1.3.0', blocker: null }, request: null };
    expect(relayAvailable()).toBe(false);
    updateRelay.value = { client: { version: '1.2.0', blocker: null }, request: null };
    releaseCheck.value = check('1.2.0', null);
    expect(relayAvailable()).toBe(false);
  });
});

describe('the relayed-update marker', () => {
  beforeEach(() => localStorage.clear());

  it('survives a reload', () => {
    saveRelayedUpdate(MARKER);
    expect(loadRelayedUpdate()).toEqual(MARKER);
    saveRelayedUpdate(null);
    expect(loadRelayedUpdate()).toBeNull();
  });

  it('drops a marker it cannot read', () => {
    for (const raw of ['not json', '{"id":"r-1"}', '42']) {
      localStorage.setItem('lucidos.relayed-update', raw);
      expect(loadRelayedUpdate()).toBeNull();
    }
  });
});
