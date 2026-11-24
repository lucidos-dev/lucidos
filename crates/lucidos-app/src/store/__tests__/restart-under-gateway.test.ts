import { describe, it, expect, beforeEach, vi } from 'vitest';
import { restartTakesPageDown } from '../progressDialogCopy';

const tauri = vi.hoisted(() => ({ value: false }));

vi.mock('../../utils/basePath', async () => {
  const actual = await vi.importActual<typeof import('../../utils/basePath')>('../../utils/basePath');
  return { ...actual, WORKSPACE_ID: 'dev' };
});
vi.mock('../../utils/platform', async () => {
  const actual = await vi.importActual<typeof import('../../utils/platform')>('../../utils/platform');
  return { ...actual, isTauri: () => tauri.value };
});

const {
  activeProgressDialog,
  connectionNoticeStatus,
  connectionStatus,
  engineRestarting,
  engineRestartNewVersion,
  enginePackaged,
  appUpdateProgress,
} = await import('../store');

beforeEach(() => {
  tauri.value = false;
  enginePackaged.value = false;
  engineRestarting.value = false;
  engineRestartNewVersion.value = false;
  appUpdateProgress.value = null;
  connectionStatus.value = 'connected';
});

describe('restartTakesPageDown', () => {
  it('is true only when the restart takes the server of this page with it', () => {
    expect(restartTakesPageDown({ servedByGateway: false, restartsGateway: false })).toBe(true);
    expect(restartTakesPageDown({ servedByGateway: true, restartsGateway: true })).toBe(true);
    expect(restartTakesPageDown({ servedByGateway: true, restartsGateway: false })).toBe(false);
  });
});

describe('an engine restart under a gateway', () => {
  it('leaves the UI usable: no blocking dialog, the banner names the restart', () => {
    engineRestarting.value = true;
    engineRestartNewVersion.value = true;
    connectionStatus.value = 'disconnected';
    expect(activeProgressDialog.value.visible).toBe(false);
    expect(connectionNoticeStatus.value).toBe('switching-version');

    engineRestartNewVersion.value = false;
    expect(connectionNoticeStatus.value).toBe('restarting');
  });

  it('hands the banner back to the connection status once the restart ends', () => {
    engineRestarting.value = true;
    engineRestarting.value = false;
    connectionStatus.value = 'connecting';
    expect(connectionNoticeStatus.value).toBe('connecting');
  });

  it('still blocks when the packaged desktop app restarts its own gateway service', () => {
    tauri.value = true;
    enginePackaged.value = true;
    engineRestarting.value = true;
    expect(activeProgressDialog.value.visible).toBe(true);
    expect(activeProgressDialog.value.title).toBe('Restarting engine');
  });

  it('keeps the packaged app update a blocking dialog', () => {
    appUpdateProgress.value = { phase: 'installing', version: '1.2.3' } as typeof appUpdateProgress.value;
    expect(activeProgressDialog.value.visible).toBe(true);
    expect(activeProgressDialog.value.title).toBe('Updating Lucidos');
  });
});
