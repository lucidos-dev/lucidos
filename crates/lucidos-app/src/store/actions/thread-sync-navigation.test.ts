import { describe, it, expect, beforeEach, vi } from 'vitest';
import { panelOverlay } from '../store';

// Mock all side-effect imports that handleNavigationRequest calls
const switchMenuItem = vi.fn();
const openSettingsSubview = vi.fn();
vi.mock('./menu', () => ({ switchMenuItem, openSettingsSubview }));

const openAppById = vi.fn();
vi.mock('./apps', () => ({
  openAppById,
  refreshAppUI: vi.fn(),
  captureAppUI: vi.fn(),
  openCredentialRequest: vi.fn(),
}));

const openFilePreview = vi.fn();
const normalizeDataPath = vi.fn((p: string) => p);
const openUrl = vi.fn();
vi.mock('./artifacts', () => ({
  loadArtifacts: vi.fn(),
  openFilePreview,
  openUrl,
  normalizeDataPath,
}));

const navigateToTrigger = vi.fn();
vi.mock('./triggers', () => ({ navigateToTrigger, loadTriggers: vi.fn() }));

const pushNavState = vi.fn();
vi.mock('./navigation', () => ({ pushNavState }));

// Minimal mocks for other imports that thread-sync.ts pulls in
vi.mock('../../api/client', () => ({
  API_BASE: '',
  postMcpConsent: vi.fn(),
}));
vi.mock('./notifications', () => ({ handleNotificationSSE: vi.fn() }));
vi.mock('./chat-changes', () => ({ syncRestartToast: vi.fn(), addRestartGroup: vi.fn() }));
vi.mock('./preferences', () => ({ loadPreferences: vi.fn() }));
vi.mock('./push', () => ({ initPushSubscription: vi.fn() }));
vi.mock('./devices', () => ({ getDeviceId: vi.fn(), toggleDevicePush: vi.fn() }));
vi.mock('../../components/chat/scrollState', () => ({ scrollToBottom: vi.fn() }));
vi.mock('./threads', () => ({ focusThread: vi.fn() }));
vi.mock('./repositories', () => ({ refreshRepoView: vi.fn() }));
vi.mock('./entityReferences', () => ({ processSSEForReferences: vi.fn() }));

const { handleNavigationRequest } = await import('./thread-sync');

describe('handleNavigationRequest', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    panelOverlay.value = null;
  });

  it('navigates to trigger details when target is "trigger" with id', () => {
    handleNavigationRequest({ target: 'trigger', id: 'task-abc-123' });
    expect(navigateToTrigger).toHaveBeenCalledWith('task-abc-123');
  });

  it('switches to triggers tab when target is "triggers" (plural)', () => {
    handleNavigationRequest({ target: 'triggers' });
    expect(switchMenuItem).toHaveBeenCalledWith('triggers');
    expect(navigateToTrigger).not.toHaveBeenCalled();
  });

  it('switches to files tab', () => {
    handleNavigationRequest({ target: 'files' });
    expect(switchMenuItem).toHaveBeenCalledWith('files');
  });

  it('opens app by id', () => {
    handleNavigationRequest({ target: 'app', app_id: 'my-app' });
    expect(openAppById).toHaveBeenCalledWith('my-app');
  });

  it('opens file preview', () => {
    handleNavigationRequest({ target: 'file', file_path: 'notes.md' });
    expect(openFilePreview).toHaveBeenCalledWith('notes.md');
  });

  it('opens URL', () => {
    handleNavigationRequest({ target: 'url', url: 'https://example.com' });
    expect(openUrl).toHaveBeenCalledWith('https://example.com');
  });

  it('opens settings with subview', () => {
    handleNavigationRequest({ target: 'settings', settings_view: 'accounts' });
    expect(switchMenuItem).toHaveBeenCalledWith('settings');
    expect(openSettingsSubview).toHaveBeenCalledWith('accounts');
  });

  it('opens new-trigger form', () => {
    handleNavigationRequest({ target: 'new-trigger' });
    expect(switchMenuItem).toHaveBeenCalledWith('triggers');
    expect(panelOverlay.value).toEqual({ type: 'form', form: { type: 'trigger' } });
  });
});
