import { describe, it, expect, beforeEach, vi } from 'vitest';
import { NAVIGATE_TARGETS, SETTINGS_VIEW_TARGETS } from '@lucidos/sdk';
import { SETTINGS_NAV_ITEMS, SETTINGS_SYSTEM_SUBPANEL_ITEMS, pluginScrollTarget } from '../store';

// Spy on showToast but keep the rest of the store real — navigation-request.ts
// reads the nav lists + settingsSubviewLabel at module load to build its
// renderable-view set, so those must be the genuine values. `vi.hoisted` is
// required because the static `import … from '../store'` above resolves the
// mocked module before plain consts initialize (TDZ).
const { showToast, setPluginsInstalledOnly } = vi.hoisted(() => ({
  showToast: vi.fn(),
  setPluginsInstalledOnly: vi.fn(),
}));
vi.mock('../store', async (importActual) => {
  const actual = await importActual<typeof import('../store')>();
  return { ...actual, showToast, setPluginsInstalledOnly };
});

// Stub every action module handleNavigationRequest dispatches into.
const openSettingsSubview = vi.fn();
const switchMenuItem = vi.fn();
const setActiveMenu = vi.fn();
vi.mock('./menu', () => ({ openSettingsSubview, switchMenuItem, setActiveMenu }));

const openAppById = vi.fn();
vi.mock('./apps', () => ({ openAppById }));

const openFilePreview = vi.fn();
const openUrl = vi.fn();
const normalizeDataPath = vi.fn((p: string) => p);
vi.mock('./artifacts', () => ({ openFilePreview, openUrl, normalizeDataPath }));

const navigateToTrigger = vi.fn();
vi.mock('./triggers', () => ({ navigateToTrigger }));

const focusThreadOrBootstrap = vi.fn();
const unfocusThread = vi.fn();
vi.mock('./threads', () => ({ focusThreadOrBootstrap, unfocusThread }));

const pushNavState = vi.fn();
vi.mock('./navigation', () => ({ pushNavState }));

const revealContentPane = vi.fn();
vi.mock('./pane', () => ({ revealContentPane }));

const ensureFocusedComposeThread = vi.fn(() => 'new-thread-id');
const updateCompose = vi.fn();
vi.mock('./compose', () => ({ ensureFocusedComposeThread, updateCompose }));

const focusPromptNow = vi.fn();
vi.mock('../../components/chat/promptFocus', () => ({ focusPromptNow }));

const { handleNavigationRequest } = await import('./navigation-request');

describe('handleNavigationRequest — settings sub-sections', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('opens a valid settings sub-section (models — the triggering case)', () => {
    handleNavigationRequest({ target: 'settings', settings_view: 'models' });
    expect(switchMenuItem).toHaveBeenCalledWith('settings');
    expect(openSettingsSubview).toHaveBeenCalledWith('models');
    expect(showToast).not.toHaveBeenCalled();
  });

  it('lands on the Settings home list when no settings_view is given', () => {
    handleNavigationRequest({ target: 'settings' });
    expect(switchMenuItem).toHaveBeenCalledWith('settings');
    expect(openSettingsSubview).not.toHaveBeenCalled();
    expect(showToast).not.toHaveBeenCalled();
  });

  it('fails loud on an unknown settings_view instead of blanking the panel', () => {
    handleNavigationRequest({ target: 'settings', settings_view: 'does-not-exist' });
    expect(switchMenuItem).toHaveBeenCalledWith('settings'); // still showed Settings
    expect(openSettingsSubview).not.toHaveBeenCalled();
    expect(showToast).toHaveBeenCalledWith(
      expect.stringContaining('Unknown settings section'),
      'error',
    );
  });
});

describe('handleNavigationRequest — plugins update deep-link', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    pluginScrollTarget.value = null;
  });

  it('lands on the installed list and focuses the carried plugin id', () => {
    handleNavigationRequest({ target: 'plugins', id: 'super-slides' });
    expect(setPluginsInstalledOnly).toHaveBeenCalledWith(true);
    expect(switchMenuItem).toHaveBeenCalledWith('plugins');
    expect(pluginScrollTarget.value).toBe('super-slides');
  });

  it('lands on the installed list without a focus target when no id is given', () => {
    handleNavigationRequest({ target: 'plugins' });
    expect(setPluginsInstalledOnly).toHaveBeenCalledWith(true);
    expect(switchMenuItem).toHaveBeenCalledWith('plugins');
    expect(pluginScrollTarget.value).toBeNull();
  });
});

// Codegen cross-checks — the generated contract (from the engine `navigate_ui`
// tool) must stay a subset of what the frontend can actually render / handle.
describe('navigate_ui contract is fully consumable by the frontend', () => {
  const renderable = new Set<string>([
    ...SETTINGS_NAV_ITEMS.map((i) => i.key),
    ...SETTINGS_SYSTEM_SUBPANEL_ITEMS.map((i) => i.key),
  ]);

  it('every advertised settings_view is a renderable Settings subview', () => {
    for (const view of SETTINGS_VIEW_TARGETS) {
      expect(renderable.has(view), `settings_view "${view}" is not renderable`).toBe(true);
    }
  });

  it('every advertised navigate target is handled (no "unknown target")', () => {
    for (const target of NAVIGATE_TARGETS) {
      vi.clearAllMocks();
      // Kitchen-sink payload so each branch finds the field it needs and never
      // toasts a missing-field error either — we only care that no branch falls
      // through to the default "Unknown navigation target" case.
      handleNavigationRequest({
        target,
        app_id: 'a',
        file_path: 'data/artifacts/x.md',
        id: 'id-1',
        url: 'https://example.com',
        settings_view: 'models',
        prompt: 'hi',
        event_id: 'e-1',
      });
      const hitDefault = showToast.mock.calls.some(
        ([msg]) => typeof msg === 'string' && msg.includes('Unknown navigation target'),
      );
      expect(hitDefault, `target "${target}" fell through to the default branch`).toBe(false);
    }
  });
});
