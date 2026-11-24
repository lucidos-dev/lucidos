import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { panelOverlay, focusedThreadId, toasts, confirmState, settleConfirm } from '../store';
import type { App } from '../types';
import { widgetFitRequests } from '../widgetFit';

const switchMenuItem = vi.fn();
const openSettingsSubview = vi.fn();
const setActiveMenu = vi.fn();
vi.mock('./menu', () => ({ switchMenuItem, openSettingsSubview, setActiveMenu, openBackupSettings: vi.fn() }));

const openAppById = vi.fn();
const exitAppFullscreen = vi.fn(() => false);
const refreshAppUI = vi.fn();
const captureAppUI = vi.fn();
vi.mock('./apps', () => ({
  openAppById,
  exitAppFullscreen,
  refreshAppUI,
  captureAppUI,
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
vi.mock('./navigation', () => ({ pushNavState, replaceNavState: vi.fn() }));

const revealContentPane = vi.fn();
const navigateToPane = vi.fn();
vi.mock('./pane', () => ({ revealContentPane, navigateToPane, holdFocusedPaneWhileTyping: vi.fn(), releaseTypingHold: vi.fn() }));

const { isMobile } = vi.hoisted(() => ({ isMobile: vi.fn(() => false) }));
vi.mock('../../utils/viewport', () => ({ isMobile }));

const unfocusThread = vi.fn();
const focusThread = vi.fn();
vi.mock('./threads', () => ({ focusThread, unfocusThread }));

const ensureFocusedComposeThread = vi.fn(() => 'new-thread-id');
const updateCompose = vi.fn();
vi.mock('./compose', () => ({ ensureFocusedComposeThread, updateCompose }));

const focusPromptNow = vi.fn();
vi.mock('../../components/chat/promptFocus', () => ({ focusPromptNow }));

vi.mock('../../api/client', () => ({
  API_BASE: '',
  API: '/api/v1',
  postMcpConsent: vi.fn(),
}));
vi.mock('./notifications', () => ({ handleNotificationSSE: vi.fn() }));
vi.mock('./chat-changes', () => ({ syncRestartState: vi.fn(), addRestartGroup: vi.fn() }));
vi.mock('./preferences', () => ({ loadPreferences: vi.fn() }));
vi.mock('./push', () => ({ setDevicePushEnabled: vi.fn() }));
const { getDeviceId } = vi.hoisted(() => ({ getDeviceId: vi.fn(() => 'this-device') }));
vi.mock('./devices', () => ({ getDeviceId }));
const openEncodedRepoFilePreview = vi.fn((path: string) => path.startsWith('repo:'));
vi.mock('./repositories', () => ({ refreshRepoView: vi.fn(), openEncodedRepoFilePreview }));
vi.mock('./entityReferences', () => ({
  processSSEForReferences: vi.fn(),
  refreshLlmConfigured: vi.fn(),
  PROVIDER_PREFERENCE_KEYS: new Set(['opencode_free_enabled', 'provider_enabled_openai']),
}));

const submitted = vi.hoisted(() => vi.fn());
vi.mock('../interactiveDeliveryService', () => ({
  submitInteractiveRequest: (request: import('../interactiveDeliveryRecords').InteractiveRequest, id?: string) => {
    submitted(request, id);
    return presentInteractiveRequest(request);
  },
}));
const { presentInteractiveRequest } = await import('./interactive-delivery');
const { handleNavigationRequest, handleThreadEvent } = await import('./thread-sync');

describe('handleNavigationRequest', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    panelOverlay.value = null;
    isMobile.mockReturnValue(false);
  });

  it('navigates to trigger details when target is "trigger" with id (delegates to navigateToTrigger, no stale-cache pre-check)', () => {
    handleNavigationRequest({ target: 'trigger', id: 'task-abc-123' });
    expect(navigateToTrigger).toHaveBeenCalledWith('task-abc-123', undefined);
  });

  it('forwards the navigate source to navigateToTrigger for a sourced error', () => {
    handleNavigationRequest({ target: 'trigger', id: 'task-abc-123' }, { source: 'thread "X"' });
    expect(navigateToTrigger).toHaveBeenCalledWith('task-abc-123', 'thread "X"');
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

  it('lands on Settings → System → Thread Queue (Thread Queue notification tap)', () => {
    handleNavigationRequest({ target: 'thread-queue' });
    expect(openSettingsSubview).toHaveBeenCalledWith('thread-queue');
    expect(switchMenuItem).not.toHaveBeenCalled();
  });

  it('opens app by id (delegates to openAppById, no stale-cache pre-check)', () => {
    handleNavigationRequest({ target: 'app', app_id: 'my-app' });
    expect(openAppById).toHaveBeenCalledWith('my-app', undefined, undefined);
  });

  it('forwards the navigate source to openAppById for a sourced error', () => {
    handleNavigationRequest({ target: 'app', app_id: 'my-app' }, { source: 'thread "X"' });
    expect(openAppById).toHaveBeenCalledWith('my-app', 'thread "X"', undefined);
  });

  it('opens file preview', () => {
    handleNavigationRequest({ target: 'file', file_path: 'notes.md' });
    expect(openFilePreview).toHaveBeenCalledWith('notes.md');
  });

  it('opens URL', () => {
    handleNavigationRequest({ target: 'url', url: 'https://example.com' });
    expect(openUrl).toHaveBeenCalledWith('https://example.com', undefined);
  });

  it('forwards the source with a URL, so a blocked tab can say who asked', () => {
    handleNavigationRequest({ target: 'url', url: 'https://example.com' }, { source: 'thread "X"' });
    expect(openUrl).toHaveBeenCalledWith('https://example.com', 'thread "X"');
  });

  it('opens settings with subview, as a single nav entry', () => {
    handleNavigationRequest({ target: 'settings', settings_view: 'accounts' });
    expect(openSettingsSubview).toHaveBeenCalledWith('accounts');
    expect(switchMenuItem).not.toHaveBeenCalled();
  });

  it('opens new-trigger form atomically (single nav push)', () => {
    handleNavigationRequest({ target: 'new-trigger' });
    expect(setActiveMenu).toHaveBeenCalledWith(
      'triggers',
      { type: 'form', form: { type: 'trigger' } },
    );
    expect(switchMenuItem).not.toHaveBeenCalled();
    expect(pushNavState).toHaveBeenCalledTimes(1);
    expect(revealContentPane).toHaveBeenCalledTimes(1);
  });

  it('opens new-app form atomically (single nav push)', () => {
    handleNavigationRequest({ target: 'new-app' });
    expect(setActiveMenu).toHaveBeenCalledWith(
      'apps',
      { type: 'form', form: { type: 'new-app' } },
    );
    expect(switchMenuItem).not.toHaveBeenCalled();
    expect(pushNavState).toHaveBeenCalledTimes(1);
    expect(revealContentPane).toHaveBeenCalledTimes(1);
  });

  it('opens fresh compose for new-chat target on a single pane, clearing the overlay (no prefill)', async () => {
    isMobile.mockReturnValue(true);
    panelOverlay.value = { type: 'file-preview', path: 'notes.md' };
    handleNavigationRequest({ target: 'new-chat' });
    expect(panelOverlay.value).toBeNull();
    expect(unfocusThread).toHaveBeenCalledTimes(1);
    expect(ensureFocusedComposeThread).toHaveBeenCalledTimes(1);
    expect(updateCompose).not.toHaveBeenCalled();
    expect(exitAppFullscreen).not.toHaveBeenCalled();
    await new Promise(resolve => requestAnimationFrame(resolve));
    expect(focusPromptNow).toHaveBeenCalledTimes(1);
  });

  it('keeps the open app on a split layout, where the conversation has its own pane', async () => {
    const overlay = { type: 'app-ui' as const, app: { id: 'demo-director', name: 'Demo Director' } as App };
    panelOverlay.value = overlay;
    handleNavigationRequest({ target: 'new-chat', prompt: 'Refresh the list' });
    expect(panelOverlay.value).toBe(overlay);
    expect(exitAppFullscreen).toHaveBeenCalledTimes(1);
    expect(revealContentPane).not.toHaveBeenCalled();
    expect(unfocusThread).toHaveBeenCalledWith();
    expect(updateCompose).toHaveBeenCalledWith('new-thread-id', { text: 'Refresh the list' });
    await new Promise(resolve => requestAnimationFrame(resolve));
    expect(focusPromptNow).toHaveBeenCalledTimes(1);
  });

  it('prefills compose draft with prompt for new-chat target', async () => {
    handleNavigationRequest({ target: 'new-chat', prompt: 'Set up a daily standup trigger' });
    expect(unfocusThread).toHaveBeenCalledTimes(1);
    expect(ensureFocusedComposeThread).toHaveBeenCalledTimes(1);
    expect(updateCompose).toHaveBeenCalledWith('new-thread-id', {
      text: 'Set up a daily standup trigger',
    });
    await new Promise(resolve => requestAnimationFrame(resolve));
    expect(focusPromptNow).toHaveBeenCalledTimes(1);
  });

  it('skips prefill for new-chat when prompt is empty string', () => {
    handleNavigationRequest({ target: 'new-chat', prompt: '' });
    expect(ensureFocusedComposeThread).toHaveBeenCalledTimes(1);
    expect(updateCompose).not.toHaveBeenCalled();
  });
});

const NIL_THREAD_ID = '00000000-0000-0000-0000-000000000000';

function navEvent(sourceThreadId: string) {
  return {
    thread_id: sourceThreadId,
    event: {
      type: 'NavigationRequested',
      payload: JSON.stringify({ target: 'app', app_id: 'demo-director' }),
    },
  };
}

function navEventFromDevice(sourceThreadId: string, deviceId: string) {
  return {
    thread_id: sourceThreadId,
    event: {
      type: 'NavigationRequested',
      payload: JSON.stringify({ target: 'app', app_id: 'demo-director' }),
      actor: { kind: 'device', device_id: deviceId },
    },
  };
}

describe('NavigationRequested scoping', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    focusedThreadId.value = null;
    toasts.value = [];
  });

  it('acts directly when the navigate comes from the focused thread', () => {
    focusedThreadId.value = 'thread-A';
    handleThreadEvent(navEvent('thread-A'));
    expect(openAppById).toHaveBeenCalledWith('demo-director', expect.any(String), undefined);
    expect(toasts.value).toHaveLength(0);
  });

  it('acts directly on a nil-thread (SDK app-iframe) navigate regardless of focus', () => {
    focusedThreadId.value = 'thread-A';
    handleThreadEvent(navEvent(NIL_THREAD_ID));
    expect(openAppById).toHaveBeenCalledWith('demo-director', 'an app', undefined);
    expect(toasts.value).toHaveLength(0);
  });

  it('offers to jump (not act) for a navigate from a non-focused thread', () => {
    focusedThreadId.value = 'thread-A';
    handleThreadEvent(navEvent('thread-B'));
    expect(openAppById).not.toHaveBeenCalled();
    expect(focusThread).not.toHaveBeenCalled();
    expect(toasts.value).toHaveLength(1);
    const offer = toasts.value[0];
    expect(offer.key).toBe('nav-offer-thread-B');
    expect(offer.action?.label).toBe('Open');
  });

  it('lands on BOTH the source thread and the target when the offer is accepted', () => {
    focusedThreadId.value = 'thread-A';
    handleThreadEvent(navEvent('thread-B'));
    toasts.value[0].action!.onClick();
    expect(focusThread).toHaveBeenCalledWith('thread-B');
    expect(openAppById).toHaveBeenCalledWith('demo-director', expect.any(String), undefined);
  });

  it('acts when the agent navigate names THIS device (focused thread)', () => {
    getDeviceId.mockReturnValue('this-device');
    focusedThreadId.value = 'thread-A';
    handleThreadEvent(navEventFromDevice('thread-A', 'this-device'));
    expect(openAppById).toHaveBeenCalledWith('demo-director', expect.any(String), undefined);
    expect(toasts.value).toHaveLength(0);
  });

  it('ignores an agent navigate naming a DIFFERENT device, even with the thread focused (no nav, no offer)', () => {
    getDeviceId.mockReturnValue('this-device');
    focusedThreadId.value = 'thread-A';
    handleThreadEvent(navEventFromDevice('thread-A', 'other-device'));
    expect(openAppById).not.toHaveBeenCalled();
    expect(toasts.value).toHaveLength(0);
  });

  it('ignores a different-device navigate from a non-focused thread (no jump offer)', () => {
    getDeviceId.mockReturnValue('this-device');
    focusedThreadId.value = 'thread-A';
    handleThreadEvent(navEventFromDevice('thread-B', 'other-device'));
    expect(openAppById).not.toHaveBeenCalled();
    expect(focusThread).not.toHaveBeenCalled();
    expect(toasts.value).toHaveLength(0);
  });

  it('still offers to jump for an actor-less navigate from a non-focused thread (trigger/background)', () => {
    getDeviceId.mockReturnValue('this-device');
    focusedThreadId.value = 'thread-A';
    handleThreadEvent(navEvent('thread-B'));
    expect(toasts.value).toHaveLength(1);
    expect(toasts.value[0].key).toBe('nav-offer-thread-B');
  });
});

describe('app capture and refresh scoping', () => {
  const OTHER = { kind: 'device', device_id: 'other-device' };
  const HERE = { kind: 'device', device_id: 'this-device' };

  function capture(actor?: object, save_format?: string) {
    return {
      thread_id: 'thread-A',
      event: { type: 'AppUiCaptureRequested', app_id: 'habit-tracker', request_id: 'req-1', actor, save_format },
    };
  }

  function refresh(actor?: object) {
    return {
      thread_id: 'thread-A',
      event: { type: 'AppUiRefreshRequested', app_id: 'habit-tracker', actor },
    };
  }

  beforeEach(() => {
    vi.clearAllMocks();
    getDeviceId.mockReturnValue('this-device');
  });

  it('captures when the request names this device', () => {
    handleThreadEvent(capture(HERE));
    expect(captureAppUI).toHaveBeenCalledWith('habit-tracker', 'req-1', undefined);
  });

  it('passes the save format on, so the frame renders a sharp file', () => {
    handleThreadEvent(capture(HERE, 'png'));
    expect(captureAppUI).toHaveBeenCalledWith('habit-tracker', 'req-1', 'png');
  });

  it('stays silent on a capture for another device, so it cannot answer first', () => {
    handleThreadEvent(capture(OTHER));
    expect(captureAppUI).not.toHaveBeenCalled();
  });

  it('captures an unscoped request, from a turn with no last used device', () => {
    handleThreadEvent(capture());
    expect(captureAppUI).toHaveBeenCalledWith('habit-tracker', 'req-1', undefined);
  });

  it('reloads the app for a refresh naming this device', () => {
    handleThreadEvent(refresh(HERE));
    expect(refreshAppUI).toHaveBeenCalledWith('habit-tracker');
  });

  it('leaves the app alone for a refresh naming another device', () => {
    handleThreadEvent(refresh(OTHER));
    expect(refreshAppUI).not.toHaveBeenCalled();
  });

  it('reloads the app for the unscoped end-of-turn refresh', () => {
    handleThreadEvent(refresh());
    expect(refreshAppUI).toHaveBeenCalledWith('habit-tracker');
  });
});

describe('push prompt scoping', () => {
  function pushRequested(actor?: object) {
    return { thread_id: 'thread-A', event: { type: 'PushNotificationRequested', actor } };
  }

  beforeEach(() => {
    vi.clearAllMocks();
    getDeviceId.mockReturnValue('this-device');
    focusedThreadId.value = 'thread-A';
  });
  afterEach(() => settleConfirm(false));

  it('asks on the device whose push preference the agent turned on', () => {
    handleThreadEvent(pushRequested({ kind: 'device', device_id: 'this-device' }));
    expect(confirmState.value.visible).toBe(true);
  });

  it('stays silent on another device showing the same thread', () => {
    handleThreadEvent(pushRequested({ kind: 'device', device_id: 'other-device' }));
    expect(confirmState.value.visible).toBe(false);
  });
});

describe('widget fit check scoping', () => {
  function fitCheck(device_id: string) {
    return {
      thread_id: 'thread-A',
      event: {
        type: 'WidgetFitRequested', app_id: 'trip', request_id: 'req-1', sizes: ['small', 'large'],
        ui_scale: 125, scale_device_id: 'the-phone', actor: { kind: 'device', device_id },
      },
    };
  }

  beforeEach(() => {
    getDeviceId.mockReturnValue('this-device');
    widgetFitRequests.value = [];
  });

  it('renders a check that names this device', () => {
    handleThreadEvent(fitCheck('this-device'));
    expect(widgetFitRequests.value).toEqual([
      { requestId: 'req-1', appId: 'trip', sizes: ['small', 'large'], scale: 125, scaleDevice: 'the-phone' },
    ]);
  });

  it('stays silent on a check for another device', () => {
    handleThreadEvent(fitCheck('other-device'));
    expect(widgetFitRequests.value).toEqual([]);
  });
});

describe('with a second window on this device', () => {
  let other: BroadcastChannel;
  let stop: () => void;

  function otherShows(state: { focusedThreadId?: string | null; appId?: string | null }): Promise<void> {
    other.postMessage({
      type: 'state',
      state: {
        id: 'other-window', focusedThreadId: null, appId: null, authorizing: null,
        activeAt: Date.now() + 1000, ...state,
      },
    });
    return new Promise((resolve) => setTimeout(resolve, 20));
  }

  beforeEach(async () => {
    vi.clearAllMocks();
    toasts.value = [];
    focusedThreadId.value = 'thread-A';
    const { installWindowPeers } = await import('../windowPeers');
    const { WORKSPACE_ID } = await import('../../utils/basePath');
    other = new BroadcastChannel(`lucidos-window-peers:${WORKSPACE_ID ?? ''}`);
    stop = installWindowPeers(() => ({
      focusedThreadId: focusedThreadId.peek(), appId: null, authorizing: null,
    }));
  });
  afterEach(() => {
    stop();
    other.close();
  });

  it('queues navigation even when cached peers nominate another window', async () => {
    await otherShows({ focusedThreadId: 'thread-A' });
    handleThreadEvent(navEvent('thread-A'));
    expect(openAppById).toHaveBeenCalledTimes(1);
  });

  it('leaves a fit check to the window showing its thread', async () => {
    getDeviceId.mockReturnValue('this-device');
    widgetFitRequests.value = [];
    focusedThreadId.value = 'thread-C';
    await otherShows({ focusedThreadId: 'thread-A' });
    handleThreadEvent({
      thread_id: 'thread-A',
      event: {
        type: 'WidgetFitRequested', app_id: 'trip', request_id: 'req-1', sizes: ['small'], ui_scale: 100,
        actor: { kind: 'device', device_id: 'this-device' },
      },
    });
    expect(widgetFitRequests.value).toEqual([]);
  });

  it('leaves a capture to the window showing the app', async () => {
    getDeviceId.mockReturnValue('this-device');
    await otherShows({ appId: 'habit-tracker' });
    handleThreadEvent({
      thread_id: 'thread-A',
      event: { type: 'AppUiCaptureRequested', app_id: 'habit-tracker', request_id: 'req-1' },
    });
    expect(captureAppUI).not.toHaveBeenCalled();
  });
});

it('threads envelope identity through automatic navigation without changing its payload', () => {
  focusedThreadId.value = 'thread-A';
  handleThreadEvent({ ...navEvent('thread-A'), event_id: 'legacy-identity' }, 'envelope-identity');
  expect(submitted).toHaveBeenLastCalledWith(expect.objectContaining({ kind: 'navigation' }), 'envelope-identity');
});

it('accepts the push prompt synchronously without waiting for an answer', () => {
  settleConfirm(false);
  const accepted = presentInteractiveRequest({ id: 'push-event', kind: 'push-enable', payload: {}, preference: { threadId: 'thread-A' } });
  expect(accepted).toBe(true);
  expect(confirmState.value.visible).toBe(true);
  settleConfirm(false);
});
