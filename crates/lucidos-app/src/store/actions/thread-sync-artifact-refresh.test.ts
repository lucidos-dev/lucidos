import { describe, it, expect, beforeEach, vi } from 'vitest';

// What refreshes an open file preview: `loadArtifacts` bumps `artifactRevision`,
// which is the cache-buster in the preview URL. These tests pin WHICH inbound
// thread events reach it, which is the whole contract that replaced the
// `refresh_file` tool the agent used to have to call by hand.
const loadArtifacts = vi.fn();
vi.mock('./artifacts', () => ({
  loadArtifacts,
  openFilePreview: vi.fn(),
  openUrl: vi.fn(),
  normalizeDataPath: vi.fn((p: string) => p),
}));

vi.mock('./menu', () => ({
  switchMenuItem: vi.fn(), openSettingsSubview: vi.fn(), setActiveMenu: vi.fn(),
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
vi.mock('./chat-changes', () => ({ syncRestartToast: vi.fn(), addRestartGroup: vi.fn() }));
vi.mock('./preferences', () => ({ loadPreferences: vi.fn() }));
vi.mock('./push', () => ({ setDevicePushEnabled: vi.fn() }));
// `./devices` is pulled in during the static import phase (via `../store`), so
// the mock fn must be vi.hoisted.
const { getDeviceId } = vi.hoisted(() => ({ getDeviceId: vi.fn(() => 'this-device') }));
vi.mock('./devices', () => ({
  getDeviceId, pendingDeviceRegistration: vi.fn(),
}));
vi.mock('../../components/chat/scrollState', () => ({ followSentMessage: vi.fn(), stopFollowingBottom: vi.fn() }));
vi.mock('./repositories', () => ({
  refreshRepoView: vi.fn(), openEncodedRepoFilePreview: vi.fn(() => false),
}));
vi.mock('./entityReferences', () => ({ processSSEForReferences: vi.fn() }));

const { handleThreadEvent } = await import('./thread-sync');

function toolResult(name: string) {
  return { thread_id: 'thread-A', event: { type: 'ToolResult', name, result: 'ok' } };
}

describe('artifact refresh on inbound thread events', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  // The file tools. These are what make an ordinary agent edit show up in an
  // open preview with no step of its own in the transcript.
  it.each(['write_file', 'edit_file', 'copy_file', 'delete_file', 'import_file'])(
    'refreshes on a %s tool result',
    (name) => {
      handleThreadEvent(toolResult(name));
      expect(loadArtifacts).toHaveBeenCalledTimes(1);
    },
  );

  // A background task (`run_bash_background` / `run_python_background`) writes
  // to `data/` unstaged by design, and emits no Artifact*/DataFile* event for
  // those writes. A drain is the only signal the frontend gets that output has
  // landed, so it has to refresh.
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

  // Plain `run_bash` is deliberately NOT a refresh trigger: its tool
  // description forbids writing to `data/`, and each entry costs a full
  // `data/` walk server-side. Pinned so re-adding it is a deliberate act.
  it('does NOT refresh on a plain run_bash tool result', () => {
    handleThreadEvent(toolResult('run_bash'));
    expect(loadArtifacts).not.toHaveBeenCalled();
  });

  it('does NOT refresh on an unrelated tool result', () => {
    handleThreadEvent(toolResult('web_search'));
    expect(loadArtifacts).not.toHaveBeenCalled();
  });

  // The retired `refresh_file` tool emitted `FileRefreshRequested`, whose only
  // job was this same `loadArtifacts()` call, always after a write path had
  // already made it. Both the tool and the event are gone; an unknown event
  // type must be inert rather than throwing.
  it('ignores the retired FileRefreshRequested event', () => {
    handleThreadEvent({
      thread_id: 'thread-A',
      event: { type: 'FileRefreshRequested', path: 'artifacts/notes.md' },
    });
    expect(loadArtifacts).not.toHaveBeenCalled();
  });
});
