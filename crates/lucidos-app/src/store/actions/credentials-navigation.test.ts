import { describe, it, expect, beforeEach, vi } from 'vitest';
import {
  panelOverlay,
  activeMenuItem,
  activeInlineForm,
  settingsSubview,
} from '../store';
import type { CredentialRequest } from '../types';

const pushNavState = vi.fn();
vi.mock('./navigation', () => ({ pushNavState }));

vi.mock('../../api/client', () => ({
  listCredentials: vi.fn().mockResolvedValue({ credentials: [] }),
  getNotifications: vi.fn().mockResolvedValue({ notifications: [], unread_count: 0, has_more: false }),
  listAppsApi: vi.fn().mockResolvedValue([]),
  listDevices: vi.fn().mockResolvedValue({ devices: [] }),
  listTriggers: vi.fn().mockResolvedValue({ triggers: [] }),
}));

const {
  openCredentialRequest,
  openAddCredential,
  openEditCredential,
} = await import('./credentials');

const helius: CredentialRequest = {
  service: 'helius',
  base_urls: ['https://api.helius.xyz'],
  auth_type: 'api_key',
  prompt: 'Paste your Helius API key.\n1. Go to https://dev.helius.xyz/dashboard\n2. Copy API Key',
};

describe('opening a credential panel pushes exactly one nav state', () => {
  beforeEach(() => {
    panelOverlay.value = null;
    activeMenuItem.value = 'files';
    settingsSubview.value = 'main';
    pushNavState.mockClear();
  });

  it('openCredentialRequest from outside settings pushes 1 nav state, not 3', () => {
    openCredentialRequest(helius);

    expect(pushNavState).toHaveBeenCalledTimes(1);
    expect(activeMenuItem.value).toBe('settings');
    expect(settingsSubview.value).toBe('accounts');
    expect(panelOverlay.value).toEqual({
      type: 'form',
      form: { type: 'credential', request: helius },
    });
  });

  it('openCredentialRequest from inside Accounts still pushes only 1 nav state', () => {
    activeMenuItem.value = 'settings';
    settingsSubview.value = 'accounts';
    pushNavState.mockClear();

    openCredentialRequest(helius);

    expect(pushNavState).toHaveBeenCalledTimes(1);
    expect(panelOverlay.value).toEqual({
      type: 'form',
      form: { type: 'credential', request: helius },
    });
  });

  it('openAddCredential pushes exactly 1 nav state', () => {
    openAddCredential();
    expect(pushNavState).toHaveBeenCalledTimes(1);
    expect(activeInlineForm.value).toEqual({ type: 'credential' });
  });

  it('openEditCredential pushes exactly 1 nav state', () => {
    openEditCredential('github');
    expect(pushNavState).toHaveBeenCalledTimes(1);
    expect(activeInlineForm.value).toEqual({ type: 'credential', editing: 'github' });
  });
});
