import { describe, it, expect, beforeEach, vi } from 'vitest';
import { activeInlineForm, panelOverlay } from '../store';
import { ApiError } from '../../api/client/_core';
import type { PluginUninstallRequest } from '../types';

// Spy on the nav-stack writes: a confirmed uninstall mutates the panel in
// place, so it must REPLACE the entry it already owns, never push a second one.
const pushNavState = vi.fn();
const replaceNavState = vi.fn();
vi.mock('./navigation', () => ({ pushNavState, replaceNavState }));

const revealContentPane = vi.fn();
vi.mock('./pane', () => ({ revealContentPane }));

// Only the three endpoints this module calls are stubbed; the rest of the
// barrel stays real so `ApiError` is the same class the 404/410 guard checks
// with `instanceof`. Arrow wrappers because `vi.mock` is hoisted above the
// `const`s and a bare reference would read them before initialization.
const confirmPluginUninstall = vi.fn();
const cancelPluginUninstall = vi.fn();
const stagePluginUninstall = vi.fn();
vi.mock('../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../api/client')>()),
  confirmPluginUninstall: (...a: unknown[]) => confirmPluginUninstall(...a),
  cancelPluginUninstall: (...a: unknown[]) => cancelPluginUninstall(...a),
  stagePluginUninstall: (...a: unknown[]) => stagePluginUninstall(...a),
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
  openPluginUninstallRequest,
  markPluginUninstalled,
  confirmPluginUninstallAction,
  cancelPluginUninstallAction,
} = await import('./plugin-uninstall');

const request: PluginUninstallRequest = {
  uninstall_id: 'u-1',
  plugin_id: 'habit-tracker',
  plugin_version: '0.3.0',
  plugin_name: 'Habit Tracker',
  files_present: ['apps/habit-tracker/manifest.json', 'apps/habit-tracker/index.html'],
  files_missing: ['knowhow/habits.md'],
};

const engineResult = {
  summary: 'Removed Habit Tracker',
  // Deliberately NOT `request.files_present`: the receipt must show what the
  // engine actually deleted, which can differ from what existed at prepare time.
  files_deleted: ['apps/habit-tracker/manifest.json'],
  files_missing: ['knowhow/habits.md', 'apps/habit-tracker/index.html'],
};

function openPending(id = 'u-1') {
  openPluginUninstallRequest({ ...request, uninstall_id: id });
  const form = activeInlineForm.value;
  if (form?.type !== 'plugin-uninstall') throw new Error('expected a plugin-uninstall form');
  return form;
}

beforeEach(() => {
  panelOverlay.value = null;
  vi.clearAllMocks();
});

describe('markPluginUninstalled', () => {
  it('turns the open panel into a receipt carrying what the engine removed', () => {
    const form = openPending();

    expect(markPluginUninstalled(form, engineResult)).toBe(true);

    const receipt = activeInlineForm.value;
    expect(receipt?.type).toBe('plugin-uninstall');
    if (receipt?.type !== 'plugin-uninstall') return;
    expect(receipt.removed?.summary).toBe('Removed Habit Tracker');
    expect(receipt.removed?.files_deleted).toEqual(engineResult.files_deleted);
    expect(receipt.removed?.files_missing).toEqual(engineResult.files_missing);
    expect(receipt.removed?.at).toBeTruthy();
    // The request rides along so the receipt still names the plugin after a
    // remount (a history round trip re-seeds the panel from the form alone).
    expect(receipt.request.plugin_name).toBe('Habit Tracker');
  });

  it('replaces the panel history entry instead of pushing a second one', () => {
    const form = openPending();
    pushNavState.mockClear();

    markPluginUninstalled(form, engineResult);

    expect(replaceNavState).toHaveBeenCalledTimes(1);
    expect(pushNavState).not.toHaveBeenCalled();
  });

  it('no-ops when the user dismissed the panel mid-uninstall', () => {
    const form = openPending();
    panelOverlay.value = null;

    expect(markPluginUninstalled(form, engineResult)).toBe(false);
    expect(panelOverlay.value).toBeNull();
    expect(replaceNavState).not.toHaveBeenCalled();
  });

  it('no-ops when a different uninstall panel took over mid-uninstall', () => {
    const form = openPending('u-1');
    // A second staged uninstall is also a `plugin-uninstall` form, so a type
    // check would let this one's receipt overwrite it. The guard is identity.
    const other = openPending('u-2');
    replaceNavState.mockClear();

    expect(markPluginUninstalled(form, engineResult)).toBe(false);
    expect(activeInlineForm.value).toBe(other);
    expect(replaceNavState).not.toHaveBeenCalled();
  });

  it('never re-stamps an existing receipt', () => {
    const form = openPending();
    markPluginUninstalled(form, engineResult);
    const receipt = activeInlineForm.value;
    if (receipt?.type !== 'plugin-uninstall') throw new Error('expected a receipt');
    replaceNavState.mockClear();

    expect(markPluginUninstalled(receipt, engineResult)).toBe(false);
    expect(activeInlineForm.value).toBe(receipt);
    expect(replaceNavState).not.toHaveBeenCalled();
  });
});

describe('confirmPluginUninstallAction', () => {
  it('leaves the receipt on screen and skips the toast', async () => {
    const form = openPending();
    confirmPluginUninstall.mockResolvedValue(engineResult);

    await confirmPluginUninstallAction(form);

    const receipt = activeInlineForm.value;
    expect(receipt?.type).toBe('plugin-uninstall');
    if (receipt?.type !== 'plugin-uninstall') return;
    expect(receipt.removed).toBeTruthy();
    // The receipt says everything the toast would, so the toast is redundant.
    expect(showToast).not.toHaveBeenCalled();
    expect(refreshPluginCatalogAfterMutation).toHaveBeenCalled();
  });

  it('falls back to a toast when the panel was dismissed mid-uninstall', async () => {
    const form = openPending();
    confirmPluginUninstall.mockImplementation(async () => {
      panelOverlay.value = null;
      return engineResult;
    });

    await confirmPluginUninstallAction(form);

    expect(panelOverlay.value).toBeNull();
    expect(showToast).toHaveBeenCalledWith(expect.stringContaining('Habit Tracker'), 'success');
  });

  it('closes the panel and reports the error when the confirm fails', async () => {
    const form = openPending();
    confirmPluginUninstall.mockRejectedValue(new Error('boom'));

    await confirmPluginUninstallAction(form);

    // The engine pops the pending entry up-front, so a failed confirm has no
    // second chance; leaving the panel open would wedge the user.
    expect(panelOverlay.value).toBeNull();
    expect(showToast).toHaveBeenCalledWith(expect.stringContaining('Uninstall failed'), 'error');
  });

  it('leaves an unrelated panel alone when a failed confirm lands late', async () => {
    const form = openPending('u-1');
    const other = openPending('u-2');
    confirmPluginUninstall.mockRejectedValue(new Error('boom'));

    await confirmPluginUninstallAction(form);

    expect(activeInlineForm.value).toBe(other);
    expect(showToast).toHaveBeenCalledWith(expect.stringContaining('Uninstall failed'), 'error');
  });
});

describe('cancelPluginUninstallAction', () => {
  it('closes its own panel and swallows an already-gone entry', async () => {
    const form = openPending();
    cancelPluginUninstall.mockRejectedValue(new ApiError(410, 'gone'));

    await cancelPluginUninstallAction(form);

    expect(panelOverlay.value).toBeNull();
    expect(showToast).not.toHaveBeenCalled();
  });

  it('leaves an unrelated panel alone when the cancel lands late', async () => {
    const form = openPending('u-1');
    const other = openPending('u-2');
    cancelPluginUninstall.mockResolvedValue(undefined);

    await cancelPluginUninstallAction(form);

    expect(activeInlineForm.value).toBe(other);
  });
});
