import { describe, it, expect, beforeEach, vi } from 'vitest';

const loadArtifacts = vi.fn();
const invalidateFilePreview = vi.fn();
vi.mock('./artifacts', () => ({
  loadArtifacts,
  invalidateFilePreview,
  openFilePreview: vi.fn(),
  openUrl: vi.fn(),
  normalizeDataPath: vi.fn((p: string) => p),
}));

vi.mock('./menu', () => ({
  switchMenuItem: vi.fn(), openSettingsSubview: vi.fn(), setActiveMenu: vi.fn(),
  openBackupSettings: vi.fn(),
}));
vi.mock('./apps', () => ({
  openAppById: vi.fn(), refreshAppUI: vi.fn(), captureAppUI: vi.fn(), openCredentialRequest: vi.fn(),
}));
vi.mock('./triggers', () => ({ navigateToTrigger: vi.fn(), loadTriggers: vi.fn() }));
vi.mock('./navigation', () => ({ pushNavState: vi.fn(), replaceNavState: vi.fn() }));
vi.mock('./pane', () => ({ revealContentPane: vi.fn(), navigateToPane: vi.fn() }));
vi.mock('./threads', () => ({ focusThread: vi.fn(), unfocusThread: vi.fn() }));
vi.mock('./compose', () => ({
  ensureFocusedComposeThread: vi.fn(() => 'new-thread-id'), updateCompose: vi.fn(),
}));
vi.mock('../../components/chat/promptFocus', () => ({ focusPromptNow: vi.fn() }));
vi.mock('../../api/client', () => ({ API_BASE: '', API: '/api/v1', postMcpConsent: vi.fn() }));
vi.mock('./notifications', () => ({ handleNotificationSSE: vi.fn() }));
vi.mock('./chat-changes', () => ({ syncRestartState: vi.fn(), addRestartGroup: vi.fn() }));
vi.mock('./preferences', () => ({ loadPreferences: vi.fn() }));
vi.mock('./push', () => ({ setDevicePushEnabled: vi.fn() }));
const { getDeviceId } = vi.hoisted(() => ({ getDeviceId: vi.fn(() => 'this-device') }));
vi.mock('./devices', () => ({
  getDeviceId,
}));
vi.mock('./repositories', () => ({
  refreshRepoView: vi.fn(), openEncodedRepoFilePreview: vi.fn(() => false),
}));
vi.mock('./entityReferences', () => ({
  processSSEForReferences: vi.fn(),
  refreshLlmConfigured: vi.fn(),
  PROVIDER_PREFERENCE_KEYS: new Set(['opencode_free_enabled', 'provider_enabled_openai']),
}));

const { handleThreadEvent } = await import('./thread-sync');

function toolResult(name: string) {
  return { thread_id: 'thread-A', event: { type: 'ToolResult', name, result: 'ok' } };
}

describe('artifact refresh on inbound thread events', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it.each(['write_file', 'edit_file', 'copy_file', 'delete_file', 'import_file'])(
    'refreshes on a %s tool result',
    (name) => {
      handleThreadEvent(toolResult(name));
      expect(loadArtifacts).toHaveBeenCalledTimes(1);
    },
  );

  it('refreshes on a bash_output drain', () => {
    handleThreadEvent(toolResult('bash_output'));
    expect(loadArtifacts).toHaveBeenCalledTimes(1);
  });

  it('refreshes when a background task completes', () => {
    handleThreadEvent({
      thread_id: 'thread-A',
      event: { type: 'BackgroundBashCompleted', task_id: 't1', command: 'sleep 1', exit_code: 0 },
    });
    expect(loadArtifacts).toHaveBeenCalledTimes(1);
  });

  it('does NOT refresh on a plain run_bash tool result', () => {
    handleThreadEvent(toolResult('run_bash'));
    expect(loadArtifacts).not.toHaveBeenCalled();
  });

  it('does NOT refresh on an unrelated tool result', () => {
    handleThreadEvent(toolResult('web_search'));
    expect(loadArtifacts).not.toHaveBeenCalled();
  });

  it('ignores the retired FileRefreshRequested event', () => {
    handleThreadEvent({
      thread_id: 'thread-A',
      event: { type: 'FileRefreshRequested', path: 'artifacts/notes.md' },
    });
    expect(loadArtifacts).not.toHaveBeenCalled();
  });
});

describe('a tool event never re-reads the open preview itself', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it.each(['write_file', 'edit_file', 'copy_file', 'delete_file', 'bash_output'])(
    'leaves the preview to the file event on a %s call and result',
    (name) => {
      handleThreadEvent({
        thread_id: 'thread-A',
        event: { type: 'ToolCalled', name, description: `Running ${name}`, args_stripped: true },
      });
      handleThreadEvent(toolResult(name));
      expect(loadArtifacts).toHaveBeenCalledTimes(1);
      expect(invalidateFilePreview).not.toHaveBeenCalled();
    },
  );

  it('leaves the preview alone when a background task completes', () => {
    handleThreadEvent({
      thread_id: 'thread-A',
      event: { type: 'BackgroundBashCompleted', task_id: 't1', command: 'sleep 1', exit_code: 0 },
    });
    expect(loadArtifacts).toHaveBeenCalledTimes(1);
    expect(invalidateFilePreview).not.toHaveBeenCalled();
  });
});
