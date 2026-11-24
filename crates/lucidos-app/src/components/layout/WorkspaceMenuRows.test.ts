import { describe, it, expect, beforeEach } from 'vitest';
import { refreshRowState, restartRowState } from './WorkspaceMenuRows';
import {
  restartRequired,
  engineVersionReady,
  enginePackaged,
  updateAvailable,
  engineNewVersionReady,
} from '../../store/store';

describe('refreshRowState', () => {
  it('nothing pending: the plain reload tooltip, no dot', () => {
    const { tooltip, showUpdateBadge } = refreshRowState(false, false);
    expect(tooltip).toBe('Reload the client');
    expect(showUpdateBadge).toBe(false);
  });

  it('client update available, no engine pending: prefixed tooltip, dot shown', () => {
    const { tooltip, showUpdateBadge } = refreshRowState(true, false);
    expect(tooltip).toBe('Update available · Reload the client');
    expect(showUpdateBadge).toBe(true);
  });

  it('client bundle rebuilt but engine still building: dot SUPPRESSED (refresh held until after the switch)', () => {
    const { tooltip, showUpdateBadge } = refreshRowState(true, true);
    expect(showUpdateBadge).toBe(false);
    expect(tooltip).toBe('Reload the client');
  });
});

describe('restartRowState', () => {
  it('nothing to switch onto: a plain restart, not highlighted, no badge', () => {
    expect(restartRowState(false)).toEqual({
      tooltip: 'Restart this workspace',
      pending: false,
      badge: null,
    });
  });

  it('a new version is ready: the row says what the restart is FOR, in words as well as colour', () => {
    expect(restartRowState(true)).toEqual({
      tooltip: 'Restart onto the new version',
      pending: true,
      badge: 'New version',
    });
  });
});

describe('engineNewVersionReady agrees with the background-build scheme', () => {
  beforeEach(() => {
    restartRequired.value = false;
    engineVersionReady.value = false;
    enginePackaged.value = false;
    updateAvailable.value = false;
  });

  it('dev: a freshly-applied restart-requiring change does NOT count until the build is ready', () => {
    restartRequired.value = true;
    engineVersionReady.value = false;
    expect(engineNewVersionReady()).toBe(false);
  });

  it('dev: counts once the background rebuild flips engineVersionReady', () => {
    restartRequired.value = true;
    engineVersionReady.value = true;
    expect(engineNewVersionReady()).toBe(true);
  });

  it('packaged: a newer release (restartRequired from the outdated check) counts immediately', () => {
    enginePackaged.value = true;
    restartRequired.value = true;
    engineVersionReady.value = false;
    expect(engineNewVersionReady()).toBe(true);
  });

  it('packaged: up to date, no signal', () => {
    enginePackaged.value = true;
    restartRequired.value = false;
    expect(engineNewVersionReady()).toBe(false);
  });
});
