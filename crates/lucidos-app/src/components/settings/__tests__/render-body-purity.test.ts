import { describe, it, expect, vi } from 'vitest';

// Mock the actions BEFORE importing the component so module-load side effects
// (if any survived the refactor) would land on these spies. The action paths
// mirror SettingsView.tsx's imports.
vi.mock('../../../store/actions/chat', async () => {
  const actual: object = await vi.importActual('../../../store/actions/chat');
  return { ...actual, loadRepositories: vi.fn() };
});
vi.mock('../../../store/actions/devices', async () => {
  const actual: object = await vi.importActual('../../../store/actions/devices');
  return { ...actual, loadDevices: vi.fn() };
});
vi.mock('../../../store/actions/credentials', async () => {
  const actual: object = await vi.importActual('../../../store/actions/credentials');
  return { ...actual, loadCredentials: vi.fn() };
});
vi.mock('../../../store/actions/oauth', async () => {
  const actual: object = await vi.importActual('../../../store/actions/oauth');
  return { ...actual, loadOAuthAccounts: vi.fn() };
});

import { loadRepositories } from '../../../store/actions/chat';
import { loadDevices } from '../../../store/actions/devices';
import { loadCredentials } from '../../../store/actions/credentials';
import { loadOAuthAccounts } from '../../../store/actions/oauth';

describe('SettingsView render-body purity', () => {
  it('does not call load actions at module import time', async () => {
    // Pre-fix: `repositoriesSection()` invoked `loadRepositories()` inside its
    // render body — a setState-in-render gotcha that preact lint flags. With
    // the fix moved into useEffect, merely importing the module must not
    // dispatch any action; the effect runs at mount time only.
    await import('../SettingsView');
    expect(loadRepositories).not.toHaveBeenCalled();
    expect(loadDevices).not.toHaveBeenCalled();
    expect(loadCredentials).not.toHaveBeenCalled();
    expect(loadOAuthAccounts).not.toHaveBeenCalled();
  });
});
