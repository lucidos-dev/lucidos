import { describe, it, expect, beforeEach } from 'vitest';
import {
  currentWorkspaceRefreshState,
  controlPanelBadgeState,
  controlPanelBadgeTooltip,
  shouldActivateConfirm,
} from './ControlPanel';
import {
  restartRequired,
  engineVersionReady,
  engineBuilding,
  enginePackaged,
  updateAvailable,
  engineNewVersionReady,
} from '../../store/store';

describe('currentWorkspaceRefreshState', () => {
  // Args: (ready, clientUpdateAvailable, enginePending)
  it('nothing pending — plain refresh tooltip, no dot', () => {
    const { tooltip, showUpdateBadge } = currentWorkspaceRefreshState(false, false, false);
    expect(tooltip).toBe('Refresh · hold to restart');
    expect(showUpdateBadge).toBe(false);
  });

  it('engine version ready, no client update — restart-and-apply tooltip, no dot', () => {
    const { tooltip, showUpdateBadge } = currentWorkspaceRefreshState(true, false, true);
    expect(tooltip).toBe('Refresh · hold to restart & apply changes');
    expect(showUpdateBadge).toBe(false);
  });

  it('client update available, no engine pending — prefixed tooltip, dot shown', () => {
    const { tooltip, showUpdateBadge } = currentWorkspaceRefreshState(false, true, false);
    expect(tooltip).toBe('Update available · Refresh · hold to restart');
    expect(showUpdateBadge).toBe(true);
  });

  it('client bundle rebuilt but engine still building — dot SUPPRESSED (refresh held until after switch)', () => {
    // The mixed-change build window: updateAvailable is true, but an engine
    // switch is pending, so the reload control must not advertise the refresh.
    const { tooltip, showUpdateBadge } = currentWorkspaceRefreshState(false, true, true);
    expect(showUpdateBadge).toBe(false);
    expect(tooltip).toBe('Refresh · hold to restart');
  });

  it('engine ready + client bundle newer — engine wins, client dot deferred', () => {
    // ready implies enginePending, so the client dot is held until after the switch.
    const { tooltip, showUpdateBadge } = currentWorkspaceRefreshState(true, true, true);
    expect(showUpdateBadge).toBe(false);
    expect(tooltip).toBe('Refresh · hold to restart & apply changes');
  });
});

describe('engineNewVersionReady — agrees with the background-build scheme', () => {
  beforeEach(() => {
    restartRequired.value = false;
    engineVersionReady.value = false;
    enginePackaged.value = false;
    updateAvailable.value = false;
  });

  it('dev: a freshly-applied restart-requiring change does NOT count until the build is ready', () => {
    // Apply time in dev: restartRequired flips true, but the background rebuild
    // hasn't finished — there is no new version to switch to yet.
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
    // Packaged has no background build — engineVersionReady never fires there.
    enginePackaged.value = true;
    restartRequired.value = true;
    engineVersionReady.value = false;
    expect(engineNewVersionReady()).toBe(true);
  });

  it('packaged: up to date — no signal', () => {
    enginePackaged.value = true;
    restartRequired.value = false;
    expect(engineNewVersionReady()).toBe(false);
  });
});

describe('shouldActivateConfirm — the Restart-confirm double-click guard', () => {
  // A long-press reveals the Cancel/Restart confirm buttons UNDER the still-down
  // pointer. The browser then pairs a stray release `click` with the long-press;
  // on the mouse path that click lands on the freshly-mounted confirm button
  // (its pointerdown was on the now-unmounted refresh glyph, before the confirm
  // rendered). The old fix — a time-fused document-level `installPairedSwallow`
  // — ate the user's FIRST deliberate click instead, forcing a double-click.
  // The confirm now fires only when the click has its OWN preceding pointerdown
  // on the button (`freshPointerDown`) or is keyboard-synthesized.

  it('stray gesture-release click (no fresh pointerdown, not keyboard) is IGNORED', () => {
    expect(shouldActivateConfirm({ freshPointerDown: false, keyboard: false })).toBe(false);
  });

  it('a genuine tap fires on the FIRST click (its own pointerdown ran on the button)', () => {
    expect(shouldActivateConfirm({ freshPointerDown: true, keyboard: false })).toBe(true);
  });

  it('a keyboard activation (Enter/Space → click with no pointerdown) fires', () => {
    expect(shouldActivateConfirm({ freshPointerDown: false, keyboard: true })).toBe(true);
  });

  it('both signals present still fires (keyboard user who also moused down)', () => {
    expect(shouldActivateConfirm({ freshPointerDown: true, keyboard: true })).toBe(true);
  });
});

describe('controlPanelBadgeState / controlPanelBadgeTooltip', () => {
  beforeEach(() => {
    restartRequired.value = false;
    engineVersionReady.value = false;
    engineBuilding.value = false;
    enginePackaged.value = false;
    updateAvailable.value = false;
  });

  it('nothing pending — no badge', () => {
    expect(controlPanelBadgeState()).toBe('none');
    expect(controlPanelBadgeTooltip()).toBeUndefined();
  });

  it('dev at Apply time (restart pending, build not ready) shows no engine badge', () => {
    restartRequired.value = true;
    expect(controlPanelBadgeState()).toBe('none');
    expect(controlPanelBadgeTooltip()).toBeUndefined();
  });

  it('a background rebuild in flight shows the spinning-build badge', () => {
    engineBuilding.value = true;
    expect(controlPanelBadgeState()).toBe('building');
    expect(controlPanelBadgeTooltip()).toBe('Building new version…');
  });

  it('building wins over a concurrently-ready signal (switch not offered until built)', () => {
    engineBuilding.value = true;
    engineVersionReady.value = true;
    updateAvailable.value = true;
    expect(controlPanelBadgeState()).toBe('building');
    expect(controlPanelBadgeTooltip()).toBe('Building new version…');
  });

  it('dev with the rebuild ready shows the attention (!) badge', () => {
    engineVersionReady.value = true;
    expect(controlPanelBadgeState()).toBe('ready');
    expect(controlPanelBadgeTooltip()).toBe('New version available');
  });

  it('engine-ready + client update available is still one attention badge', () => {
    engineVersionReady.value = true;
    updateAvailable.value = true;
    expect(controlPanelBadgeState()).toBe('ready');
    expect(controlPanelBadgeTooltip()).toBe('New version available · Client update available');
  });

  it('client update alone (engine idle) shows the attention badge with the client tooltip', () => {
    updateAvailable.value = true;
    expect(controlPanelBadgeState()).toBe('ready');
    expect(controlPanelBadgeTooltip()).toBe('Client update available');
  });
});
