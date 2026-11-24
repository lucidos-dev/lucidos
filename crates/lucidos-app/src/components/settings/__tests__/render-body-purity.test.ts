import '../../../utils/renderMarkdown';
import { describe, it, expect, vi } from 'vitest';

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
    await import('../SettingsView');
    expect(loadRepositories).not.toHaveBeenCalled();
    expect(loadDevices).not.toHaveBeenCalled();
    expect(loadCredentials).not.toHaveBeenCalled();
    expect(loadOAuthAccounts).not.toHaveBeenCalled();
  }, 30_000);
});
