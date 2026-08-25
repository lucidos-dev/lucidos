import { describe, it, expect, beforeEach, vi } from 'vitest';

// Two contracts, deliberately apart. `loadArtifacts` refreshes the Files LIST,
// and these tests pin which inbound thread events reach it: that is what
// replaced the `refresh_file` tool the agent used to have to call by hand.
// `invalidateFilePreview` re-reads the OPEN PREVIEW, and takes a path, because
// a write to another file must not restart a video the user is watching.
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

// A `ToolResult` carries no path, and the engine announces one only for
// `artifacts/` writes. So the path comes from the paired `ToolCalled`, which is
// the only way an open `knowhow/` or `apps/` preview learns its file changed.
describe('the open preview re-reads only the path the tool wrote', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  function toolCalled(name: string, args: Record<string, unknown>, eventId?: string) {
    return { thread_id: 'thread-A', event_id: eventId, event: { type: 'ToolCalled', name, args } };
  }

  function toolResultPairedWith(name: string, toolCalledEventId?: string) {
    return {
      thread_id: 'thread-A',
      event: { type: 'ToolResult', name, result: 'ok', tool_called_event_id: toolCalledEventId },
    };
  }

  it.each([
    ['write_file', { path: 'knowhow/ops/deploy.md' }, 'knowhow/ops/deploy.md'],
    ['edit_file', { path: 'apps/demo/index.html' }, 'apps/demo/index.html'],
    ['delete_file', { path: 'artifacts/old.md' }, 'artifacts/old.md'],
    ['copy_file', { source: 'a.md', destination: 'artifacts/b.md' }, 'artifacts/b.md'],
    ['edit_file', { path: 'knowhow/ops/deploy.md', commit: true }, 'knowhow/ops/deploy.md'],
  ])('offers %s its written path', (name, args, expected) => {
    handleThreadEvent(toolCalled(name, args));
    handleThreadEvent(toolResultPairedWith(name));
    expect(invalidateFilePreview).toHaveBeenCalledWith(expected);
  });

  // The one signal the drain gives is that SOMETHING landed. Invalidating on
  // it is exactly the blanket reload this replaced.
  it('leaves the preview alone on a bash_output drain', () => {
    handleThreadEvent(toolResultPairedWith('bash_output'));
    expect(loadArtifacts).toHaveBeenCalledTimes(1);
    expect(invalidateFilePreview).not.toHaveBeenCalled();
  });

  it('leaves the preview alone when a background task completes', () => {
    handleThreadEvent({
      thread_id: 'thread-A',
      event: { type: 'BackgroundBashCompleted', task_id: 't1', command: 'sleep 1', exit_code: 0 },
    });
    expect(loadArtifacts).toHaveBeenCalledTimes(1);
    expect(invalidateFilePreview).not.toHaveBeenCalled();
  });

  it('offers nothing when the tool named no path', () => {
    handleThreadEvent(toolCalled('write_file', {}));
    handleThreadEvent(toolResultPairedWith('write_file'));
    expect(invalidateFilePreview).not.toHaveBeenCalled();
  });

  // `import_file`'s destination is relative to `artifacts/imported/`, not to
  // `data/`, and is optional. `ArtifactImported` announces the path the engine
  // settled on, so that event is what refreshes an imported file.
  it('offers nothing for import_file, whose destination is in another frame', () => {
    handleThreadEvent(toolCalled('import_file', { source_path: '/tmp/x.png', destination: 'x.png' }));
    handleThreadEvent(toolResultPairedWith('import_file'));
    expect(invalidateFilePreview).not.toHaveBeenCalled();
  });

  // With `repo`, an `edit_file` path is relative to that repository's root and
  // the edit writes nothing under `data/`. Reading it as a data path would
  // attribute a repo file to the data tree.
  it('offers nothing for a repo-scoped edit_file', () => {
    handleThreadEvent(toolCalled('edit_file', { path: 'src/main.rs', repo: 'r1', commit: false }));
    handleThreadEvent(toolResultPairedWith('edit_file'));
    expect(invalidateFilePreview).not.toHaveBeenCalled();
  });

  // A tool between the call and its result must not let the recorded path be
  // attributed to whatever finishes next.
  it('drops a recorded path when another tool result arrives first', () => {
    handleThreadEvent(toolCalled('write_file', { path: 'artifacts/a.md' }));
    handleThreadEvent(toolResultPairedWith('web_search'));
    handleThreadEvent(toolResultPairedWith('bash_output'));
    expect(invalidateFilePreview).not.toHaveBeenCalled();
  });

  it('drops a recorded path when the result pairs with a different call', () => {
    handleThreadEvent(toolCalled('write_file', { path: 'artifacts/a.md' }, 'call-1'));
    handleThreadEvent(toolResultPairedWith('write_file', 'call-2'));
    expect(invalidateFilePreview).not.toHaveBeenCalled();
  });

  it('keeps only the newest call, so a superseded path is never used', () => {
    handleThreadEvent(toolCalled('write_file', { path: 'artifacts/a.md' }));
    handleThreadEvent(toolCalled('write_file', { path: 'artifacts/b.md' }));
    handleThreadEvent(toolResultPairedWith('write_file'));
    expect(invalidateFilePreview).toHaveBeenCalledTimes(1);
    expect(invalidateFilePreview).toHaveBeenCalledWith('artifacts/b.md');
  });
});
