import { describe, it, expect, beforeEach, vi } from 'vitest';
import { activeInlineForm, panelOverlay } from '../store';
import type { PluginInstallRequest } from '../types';

const pushNavState = vi.fn();
const replaceNavState = vi.fn();
vi.mock('./navigation', () => ({ pushNavState, replaceNavState }));

const revealContentPane = vi.fn();
vi.mock('./pane', () => ({ revealContentPane }));

const focusSpawnedThread = vi.fn();
vi.mock('./threads', () => ({ focusSpawnedThread }));

const confirmPluginInstall = vi.fn();
const cancelPluginInstall = vi.fn();
const stagePluginInstall = vi.fn();
const proposePluginUpstream = vi.fn();
vi.mock('../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../api/client')>()),
  confirmPluginInstall: (...a: unknown[]) => confirmPluginInstall(...a),
  cancelPluginInstall: (...a: unknown[]) => cancelPluginInstall(...a),
  stagePluginInstall: (...a: unknown[]) => stagePluginInstall(...a),
  proposePluginUpstream: (...a: unknown[]) => proposePluginUpstream(...a),
}));

const refreshPluginCatalogAfterMutation = vi.fn();
vi.mock('./plugin-marketplaces', () => ({
  refreshPluginCatalogAfterMutation: (...a: unknown[]) => refreshPluginCatalogAfterMutation(...a),
}));

const showToast = vi.fn();
vi.mock('../store', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../store')>()),
  showToast: (...a: unknown[]) => showToast(...a),
}));

const {
  openPluginInstallRequest,
  markPluginInstalled,
  confirmPluginInstallAction,
  proposePluginUpstreamAction,
} = await import('./plugin-install');

const request: PluginInstallRequest = {
  install_id: 'i-1',
  source: 'https://github.com/example-org/example-repo',
  source_type: 'git',
  manifest: { description: 'Tracks habits.' },
  files: ['apps/habit-tracker/manifest.json', 'apps/habit-tracker/index.html'],
  overwrites: [],
  plugin_id: 'habit-tracker',
  plugin_version: '0.3.0',
  plugin_name: 'Habit Tracker',
};

const engineResult = {
  summary: 'Installed Habit Tracker',
  installed_files: ['apps/habit-tracker/manifest.json'],
};

function openPending(id = 'i-1') {
  openPluginInstallRequest({ ...request, install_id: id });
  const form = activeInlineForm.value;
  if (form?.type !== 'plugin-install') throw new Error('expected a plugin-install form');
  return form;
}

beforeEach(() => {
  panelOverlay.value = null;
  vi.clearAllMocks();
});

describe('markPluginInstalled', () => {
  it('turns the open panel into a receipt carrying what the engine wrote', () => {
    const form = openPending();

    expect(markPluginInstalled(form, engineResult)).toBe(true);

    const receipt = activeInlineForm.value;
    expect(receipt?.type).toBe('plugin-install');
    if (receipt?.type !== 'plugin-install') return;
    expect(receipt.installed?.summary).toBe('Installed Habit Tracker');
    expect(receipt.installed?.installed_files).toEqual(engineResult.installed_files);
    expect(receipt.installed?.at).toBeTruthy();
    expect(receipt.request.plugin_name).toBe('Habit Tracker');
  });

  it('carries the triggers that rely on a file the update removed', () => {
    const form = openPending();
    const affected = [{
      trigger_id: 't-1',
      trigger_name: 'Daily habit check',
      paths: [{ path: 'apps/habit-tracker/knowhow/daily.md', reason: 'owning-app' as const }],
    }];

    markPluginInstalled(form, { ...engineResult, affected_triggers: affected });

    const receipt = activeInlineForm.value;
    if (receipt?.type !== 'plugin-install') throw new Error('expected a receipt');
    expect(receipt.installed?.affected_triggers).toEqual(affected);
  });

  it('replaces the panel history entry instead of pushing a second one', () => {
    const form = openPending();
    pushNavState.mockClear();

    markPluginInstalled(form, engineResult);

    expect(replaceNavState).toHaveBeenCalledTimes(1);
    expect(pushNavState).not.toHaveBeenCalled();
  });

  it('no-ops when the user dismissed the panel mid-install', () => {
    const form = openPending();
    panelOverlay.value = null;

    expect(markPluginInstalled(form, engineResult)).toBe(false);
    expect(panelOverlay.value).toBeNull();
    expect(replaceNavState).not.toHaveBeenCalled();
  });

  it('no-ops when a different install panel took over mid-install', () => {
    const form = openPending('i-1');
    const other = openPending('i-2');
    replaceNavState.mockClear();

    expect(markPluginInstalled(form, engineResult)).toBe(false);
    expect(activeInlineForm.value).toBe(other);
    expect(replaceNavState).not.toHaveBeenCalled();
  });

  it('never re-stamps an existing receipt', () => {
    const form = openPending();
    markPluginInstalled(form, engineResult);
    const receipt = activeInlineForm.value;
    if (receipt?.type !== 'plugin-install') throw new Error('expected a receipt');
    replaceNavState.mockClear();

    expect(markPluginInstalled(receipt, engineResult)).toBe(false);
    expect(activeInlineForm.value).toBe(receipt);
    expect(replaceNavState).not.toHaveBeenCalled();
  });
});

describe('confirmPluginInstallAction', () => {
  it('leaves the receipt on screen and skips the toast', async () => {
    const form = openPending();
    confirmPluginInstall.mockResolvedValue(engineResult);

    await confirmPluginInstallAction(form);

    const receipt = activeInlineForm.value;
    expect(receipt?.type).toBe('plugin-install');
    if (receipt?.type !== 'plugin-install') return;
    expect(receipt.installed).toBeTruthy();
    expect(showToast).not.toHaveBeenCalled();
    expect(focusSpawnedThread).not.toHaveBeenCalled();
    expect(refreshPluginCatalogAfterMutation).toHaveBeenCalled();
  });

  it('still jumps into the setup thread, and keeps the receipt too', async () => {
    const form = openPending();
    confirmPluginInstall.mockResolvedValue({ ...engineResult, setup_thread_id: 't-9' });

    await confirmPluginInstallAction(form);

    expect(focusSpawnedThread).toHaveBeenCalledWith('t-9');
    const receipt = activeInlineForm.value;
    expect(receipt?.type).toBe('plugin-install');
    if (receipt?.type !== 'plugin-install') return;
    expect(receipt.installed).toBeTruthy();
    expect(showToast).not.toHaveBeenCalled();
  });

  it('reports a failed jump to the setup thread as a navigation failure, not an install failure', async () => {
    const form = openPending();
    confirmPluginInstall.mockResolvedValue({ ...engineResult, setup_thread_id: 't-9' });
    focusSpawnedThread.mockImplementation(() => { throw new Error('boom'); });

    await expect(confirmPluginInstallAction(form)).resolves.toBeUndefined();

    expect(showToast).toHaveBeenCalledWith(
      expect.stringContaining("Installed Habit Tracker v0.3.0, but couldn't open its setup thread"),
      'error',
    );
    const receipt = activeInlineForm.value;
    expect(receipt?.type).toBe('plugin-install');
    if (receipt?.type !== 'plugin-install') return;
    expect(receipt.installed).toBeTruthy();
  });

  it('falls back to a toast when the panel was dismissed mid-install', async () => {
    const form = openPending();
    confirmPluginInstall.mockImplementation(async () => {
      panelOverlay.value = null;
      return engineResult;
    });

    await confirmPluginInstallAction(form);

    expect(panelOverlay.value).toBeNull();
    expect(showToast).toHaveBeenCalledWith('Installed Habit Tracker v0.3.0', 'success');
  });

  it('closes the panel and reports the error when the confirm fails', async () => {
    const form = openPending();
    confirmPluginInstall.mockRejectedValue(new Error('boom'));

    await confirmPluginInstallAction(form);

    expect(panelOverlay.value).toBeNull();
    expect(showToast).toHaveBeenCalledWith(expect.stringContaining('Install failed'), 'error');
  });

  it('leaves an unrelated panel alone when a failed confirm lands late', async () => {
    const form = openPending('i-1');
    const other = openPending('i-2');
    confirmPluginInstall.mockRejectedValue(new Error('boom'));

    await confirmPluginInstallAction(form);

    expect(activeInlineForm.value).toBe(other);
  });
});

describe('proposePluginUpstreamAction', () => {
  it('reports a failed jump to the new thread as a navigation failure, not a failed proposal', async () => {
    proposePluginUpstream.mockResolvedValue({ thread_id: 't-7', patch_path: 'patches/habit-tracker.patch' });
    focusSpawnedThread.mockImplementationOnce(() => { throw new Error('boom'); });

    await proposePluginUpstreamAction('habit-tracker', 'Habit Tracker');

    expect(showToast).toHaveBeenCalledWith(
      expect.stringContaining('Saved your Habit Tracker patch to data/patches/habit-tracker.patch'),
      'error',
    );
    expect(showToast).not.toHaveBeenCalledWith(expect.stringContaining("Couldn't prepare a patch"), 'error');
  });
});
