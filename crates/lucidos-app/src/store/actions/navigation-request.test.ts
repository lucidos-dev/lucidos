import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { NAVIGATE_TARGETS, SETTINGS_VIEW_TARGETS } from '@lucidos/sdk';
import { SETTINGS_SUBVIEW_ITEMS, pluginScrollTarget, selectedLines, lineScrollTarget, filePreviewSource, panelOverlay, repositories, findRequest, focusedThreadId, settingsLinkedFragment } from '../store';

const { showToast, setPluginsInstalledOnly } = vi.hoisted(() => ({
  showToast: vi.fn(),
  setPluginsInstalledOnly: vi.fn(),
}));
vi.mock('../store', async (importActual) => {
  const actual = await importActual<typeof import('../store')>();
  return { ...actual, showToast, setPluginsInstalledOnly };
});

const openSettingsSubview = vi.fn();
const switchMenuItem = vi.fn();
const setActiveMenu = vi.fn();
vi.mock('./menu', () => ({ openSettingsSubview, switchMenuItem, setActiveMenu }));

const openAppById = vi.fn();
const exitAppFullscreen = vi.fn(() => false);
const refreshApps = vi.fn(async () => {});
vi.mock('./apps', () => ({ openAppById, exitAppFullscreen, refreshApps }));

const openFilePreview = vi.fn();
const openUrl = vi.fn();
const normalizeDataPath = vi.fn((p: string) => p);
vi.mock('./artifacts', () => ({ openFilePreview, openUrl, normalizeDataPath }));

const openOAuthAuthorizationUrl = vi.fn();
vi.mock('./oauth', () => ({ openOAuthAuthorizationUrl }));

const navigateToTrigger = vi.fn();
vi.mock('./triggers', () => ({ navigateToTrigger }));

const focusThreadOrBootstrap = vi.fn();
const unfocusThread = vi.fn();
vi.mock('./threads', () => ({ focusThreadOrBootstrap, unfocusThread }));

const pushNavState = vi.fn();
vi.mock('./navigation', () => ({ pushNavState }));

const revealContentPane = vi.fn();
vi.mock('./pane', () => ({ revealContentPane, showDashboardRail: vi.fn() }));

const ensureFocusedComposeThread = vi.fn(() => 'new-thread-id');
const updateCompose = vi.fn();
const applyDestination = vi.fn();
const updateComposeSelection = vi.fn();
vi.mock('./compose', () => ({ ensureFocusedComposeThread, updateCompose, applyDestination, updateComposeSelection }));

const focusPromptNow = vi.fn();
vi.mock('../../components/chat/promptFocus', () => ({ focusPromptNow }));

const openEncodedRepoFilePreview = vi.fn((path: string) => path.startsWith('repo:'));
vi.mock('./repositories', () => ({ openEncodedRepoFilePreview }));

const { refreshRepositories } = vi.hoisted(() => ({ refreshRepositories: vi.fn(async () => {}) }));
vi.mock('./repositoriesLoader', () => ({ refreshRepositories }));

const { handleNavigationRequest, describeNavTarget } = await import('./navigation-request');
const { fileFindScope } = await import('./find-request');

describe('handleNavigationRequest — settings sub-sections', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('opens a valid settings sub-section (models — the triggering case)', () => {
    handleNavigationRequest({ target: 'settings', settings_view: 'models' });
    expect(openSettingsSubview).toHaveBeenCalledWith('models');
    expect(showToast).not.toHaveBeenCalled();
  });

  it('lands a sub-section in ONE nav push, with no Settings-home entry first', () => {
    handleNavigationRequest({ target: 'settings', settings_view: 'backup' });
    expect(openSettingsSubview).toHaveBeenCalledWith('backup');
    expect(switchMenuItem).not.toHaveBeenCalled();
  });

  it('hands a settings fragment to the page it opens', () => {
    handleNavigationRequest({ target: 'settings', settings_view: 'providers', fragment: 'xai' });
    expect(openSettingsSubview).toHaveBeenCalledWith('providers');
    expect(settingsLinkedFragment.value).toBe('xai');
    expect(showToast).not.toHaveBeenCalled();
  });

  it('names the page alone with no fragment', () => {
    settingsLinkedFragment.value = null;
    handleNavigationRequest({ target: 'settings', settings_view: 'providers' });
    expect(settingsLinkedFragment.value).toBeNull();
  });

  it('opens the aliased view for a fragment', () => {
    handleNavigationRequest({ target: 'settings', settings_view: 'mobile-access', fragment: 'addresses' });
    expect(openSettingsSubview).toHaveBeenCalledWith('access');
    expect(settingsLinkedFragment.value).toBe('addresses');
  });

  it('ignores a fragment on an unknown view', () => {
    settingsLinkedFragment.value = null;
    handleNavigationRequest({ target: 'settings', settings_view: 'not-a-section', fragment: 'xai' });
    expect(settingsLinkedFragment.value).toBeNull();
  });

  it('lands the Thread Queue subpanel in ONE nav push too', () => {
    handleNavigationRequest({ target: 'thread-queue' });
    expect(openSettingsSubview).toHaveBeenCalledWith('thread-queue');
    expect(switchMenuItem).not.toHaveBeenCalled();
  });

  it('aliases a retired settings_view onto the category that absorbed it', () => {
    for (const [sent, expected] of [
      ['repositories', 'coding-agents'],
      ['mobile-access', 'access'],
      ['network-access', 'access'],
      ['links', 'appearance'],
      ['experimental', 'appearance'],
    ]) {
      vi.clearAllMocks();
      handleNavigationRequest({ target: 'settings', settings_view: sent });
      expect(openSettingsSubview, `"${sent}" should land on "${expected}"`).toHaveBeenCalledWith(expected);
      expect(switchMenuItem, `"${sent}" should not also push Settings home`).not.toHaveBeenCalled();
      expect(showToast).not.toHaveBeenCalled();
    }
  });

  it('still reports a settings_view that is neither live nor retired', () => {
    handleNavigationRequest({ target: 'settings', settings_view: 'not-a-section' });
    expect(openSettingsSubview).not.toHaveBeenCalled();
    expect(showToast).toHaveBeenCalledWith('Unknown settings section: "not-a-section"', 'error');
  });

  it('lands on the Settings home list when no settings_view is given', () => {
    handleNavigationRequest({ target: 'settings' });
    expect(switchMenuItem).toHaveBeenCalledWith('settings');
    expect(openSettingsSubview).not.toHaveBeenCalled();
    expect(showToast).not.toHaveBeenCalled();
  });

  it('fails loud on an unknown settings_view instead of blanking the panel', () => {
    handleNavigationRequest({ target: 'settings', settings_view: 'does-not-exist' });
    expect(switchMenuItem).toHaveBeenCalledWith('settings');
    expect(openSettingsSubview).not.toHaveBeenCalled();
    expect(showToast).toHaveBeenCalledWith(
      expect.stringContaining('Unknown settings section'),
      'error',
    );
  });
});

describe('handleNavigationRequest — new-chat destination prefill', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    repositories.value = { status: 'loaded', data: [{ id: 'repo-1', name: 'example-repo', path: '/repos/example-repo' }] };
    focusedThreadId.value = 'new-thread-id';
  });

  afterEach(() => {
    repositories.value = { status: 'not-loaded' };
    focusedThreadId.value = null;
  });

  it('leaves the destination alone for a bare new-chat', () => {
    expect(handleNavigationRequest({ target: 'new-chat', prompt: 'hello' })).toBeUndefined();
    expect(updateCompose).toHaveBeenCalledWith('new-thread-id', { text: 'hello' });
    expect(applyDestination).not.toHaveBeenCalled();
    expect(updateComposeSelection).not.toHaveBeenCalled();
  });

  it('aims the NEW draft at the folder and agent, and keeps the prompt', () => {
    handleNavigationRequest(
      { target: 'new-chat', prompt: 'Work on PR #123', coding_agent: 'codex', folder: 'example-repo' },
      { source: 'an app' },
    );
    expect(updateCompose).toHaveBeenCalledWith('new-thread-id', { text: 'Work on PR #123' });
    expect(applyDestination).toHaveBeenCalledWith('new-thread-id', { kind: 'coding', scope: { kind: 'external', repoId: 'repo-1' } });
    expect(updateComposeSelection).toHaveBeenCalledWith('new-thread-id', { codingAgent: 'codex' });
    expect(showToast).not.toHaveBeenCalled();
  });

  it('opens the chat with its prompt and warns when the folder names nothing', async () => {
    const pending = handleNavigationRequest(
      { target: 'new-chat', prompt: 'Work on PR #123', folder: 'no-such-repo' },
      { source: 'an app' },
    );
    expect(pending).toBeInstanceOf(Promise);
    await pending;
    expect(updateCompose).toHaveBeenCalledWith('new-thread-id', { text: 'Work on PR #123' });
    expect(applyDestination).not.toHaveBeenCalled();
    expect(showToast).toHaveBeenCalledWith(expect.stringContaining('requested by an app'), 'error');
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

  it('drops an earlier unconsumed focus target when no id is given', () => {
    pluginScrollTarget.value = 'super-slides';
    handleNavigationRequest({ target: 'plugins' });
    expect(pluginScrollTarget.value).toBeNull();
  });
});

describe('handleNavigationRequest — file target', () => {
  const REPO_ID = '3f9c1b2e-0d44-4a71-9f6d-2e5b8c7a1d03';
  const ENCODED = `repo:${REPO_ID}:file:src/main/resources/transforms/x.jslt`;

  beforeEach(() => {
    vi.clearAllMocks();
    selectedLines.value = null;
    lineScrollTarget.value = null;
    filePreviewSource.value = false;
  });

  it('routes a data-tree path to the workspace file preview', () => {
    handleNavigationRequest({ target: 'file', file_path: 'artifacts/notes.md' });
    expect(openFilePreview).toHaveBeenCalledWith('artifacts/notes.md');
  });

  describe('with a find', () => {
    beforeEach(() => {
      findRequest.value = null;
    });

    it('asks Find in file to open on the opened file, from the cited line', () => {
      handleNavigationRequest({ target: 'file', file_path: 'artifacts/notes.md', line: 4 }, { find: 'zqxv needle' });
      expect(findRequest.value).toEqual({ scope: fileFindScope('artifacts/notes.md'), query: 'zqxv needle', line: 4 });
      expect(lineScrollTarget.value, 'the line landing still happens').toEqual({ path: 'artifacts/notes.md', line: 4 });
    });

    it('asks for nothing where the file cannot show the line', () => {
      handleNavigationRequest({ target: 'file', file_path: 'artifacts/photo.png', line: 4 }, { find: 'zqxv needle' });
      expect(findRequest.value).toBeNull();
    });
  });

  it('routes a repo-encoded path to the repo preview, intact', () => {
    handleNavigationRequest({ target: 'file', file_path: ENCODED });
    expect(openEncodedRepoFilePreview).toHaveBeenCalledWith(ENCODED);
    expect(openFilePreview).not.toHaveBeenCalled();
  });

  describe('a repository named by its name', () => {
    afterEach(() => {
      repositories.value = { status: 'not-loaded' };
    });

    it('opens the file under the repository id, line and all', async () => {
      repositories.value = { status: 'loaded', data: [{ id: REPO_ID, name: 'example-repo', path: '/src/example' }] };
      handleNavigationRequest({ target: 'file', file_path: 'repo:example-repo:file#main:docs/x.md', line: 7 });
      await vi.waitFor(() => expect(openEncodedRepoFilePreview).toHaveBeenCalledWith(`repo:${REPO_ID}:file#main:docs/x.md`));
      expect(openEncodedRepoFilePreview).not.toHaveBeenCalledWith(expect.stringContaining('example-repo'));
      expect(selectedLines.value).toEqual({ start: 7, end: 7 });
    });

    it('resolves a name on a cold launch, before the repository list has loaded', async () => {
      refreshRepositories.mockImplementationOnce(async () => {
        repositories.value = { status: 'loaded', data: [{ id: REPO_ID, name: 'example-repo', path: '/src/example' }] };
      });
      await handleNavigationRequest({ target: 'file', file_path: 'repo:example-repo:file:docs/x.md' });
      expect(openEncodedRepoFilePreview).toHaveBeenCalledWith(`repo:${REPO_ID}:file:docs/x.md`);
    });

    it('opens a uuid as written while the list is still loading', () => {
      handleNavigationRequest({ target: 'file', file_path: ENCODED });
      expect(refreshRepositories).not.toHaveBeenCalled();
      expect(openEncodedRepoFilePreview).toHaveBeenCalledWith(ENCODED);
    });

    it('opens a known id as written', () => {
      repositories.value = { status: 'loaded', data: [{ id: REPO_ID, name: 'example-repo', path: '/src/example' }] };
      handleNavigationRequest({ target: 'file', file_path: ENCODED });
      expect(openEncodedRepoFilePreview).toHaveBeenCalledTimes(1);
      expect(openEncodedRepoFilePreview).toHaveBeenCalledWith(ENCODED);
    });
  });

  it('toasts and opens nothing when file_path is missing', () => {
    handleNavigationRequest({ target: 'file' });
    expect(openFilePreview).not.toHaveBeenCalled();
    expect(openEncodedRepoFilePreview).not.toHaveBeenCalled();
    expect(showToast).toHaveBeenCalledWith('Navigation target missing file_path', 'error');
  });

  it('describes a repo-encoded destination by its repo-relative path', () => {
    expect(describeNavTarget({ target: 'file', file_path: ENCODED }))
      .toBe('file "src/main/resources/transforms/x.jslt"');
  });

  it('describes a data-tree destination by its path', () => {
    expect(describeNavTarget({ target: 'file', file_path: 'artifacts/notes.md' }))
      .toBe('file "artifacts/notes.md"');
  });

  it('names the cited line in the jump offer, the way the citation did', () => {
    expect(describeNavTarget({ target: 'file', file_path: ENCODED, line: 510, line_end: 520 }))
      .toBe('file "src/main/resources/transforms/x.jslt:510"');
  });

  it('leaves the offer unqualified when the line is unusable', () => {
    expect(describeNavTarget({ target: 'file', file_path: 'artifacts/notes.md', line: 0 }))
      .toBe('file "artifacts/notes.md"');
  });
});

describe('handleNavigationRequest: file target at a line', () => {
  const ENCODED = 'repo:3f9c1b2e-0d44-4a71-9f6d-2e5b8c7a1d03:file:src/main.rs';

  beforeEach(() => {
    vi.clearAllMocks();
    selectedLines.value = null;
    lineScrollTarget.value = null;
    filePreviewSource.value = false;
  });

  it('selects and scrolls to a single line in a repo file', () => {
    handleNavigationRequest({ target: 'file', file_path: ENCODED, line: 510 });

    expect(openEncodedRepoFilePreview).toHaveBeenCalledWith(ENCODED);
    expect(selectedLines.value).toEqual({ start: 510, end: 510 });
    expect(lineScrollTarget.value, 'named by the path the preview shows').toEqual({ path: ENCODED, line: 510 });
    expect(filePreviewSource.value).toBe(true);
  });

  it('selects a range in a workspace data file', () => {
    handleNavigationRequest({ target: 'file', file_path: 'artifacts/notes.md', line: 10, line_end: 20 });

    expect(openFilePreview).toHaveBeenCalledWith('artifacts/notes.md');
    expect(selectedLines.value).toEqual({ start: 10, end: 20 });
    expect(lineScrollTarget.value).toEqual({ path: 'artifacts/notes.md', line: 10 });
    expect(filePreviewSource.value).toBe(true);
  });

  it('is a no-op with no line: the file opens at the top, unselected', () => {
    handleNavigationRequest({ target: 'file', file_path: ENCODED });

    expect(openEncodedRepoFilePreview).toHaveBeenCalledWith(ENCODED);
    expect(selectedLines.value).toBeNull();
    expect(lineScrollTarget.value).toBeNull();
    expect(filePreviewSource.value).toBe(false);
  });

  it.each([
    ['zero', 0],
    ['negative', -3],
    ['fractional', 1.5],
    ['a string', 'abc'],
  ])('still opens the file when the line is %s', (_label, line) => {
    handleNavigationRequest({ target: 'file', file_path: 'artifacts/notes.md', line } as {
      target: string; file_path: string; line?: number;
    });

    expect(openFilePreview).toHaveBeenCalledWith('artifacts/notes.md');
    expect(selectedLines.value).toBeNull();
    expect(lineScrollTarget.value).toBeNull();
    expect(showToast).not.toHaveBeenCalled();
  });

  it('accepts a line past the end of the file without complaint', () => {
    handleNavigationRequest({ target: 'file', file_path: 'artifacts/notes.md', line: 9_000_000 });

    expect(openFilePreview).toHaveBeenCalledWith('artifacts/notes.md');
    expect(selectedLines.value).toEqual({ start: 9_000_000, end: 9_000_000 });
    expect(showToast).not.toHaveBeenCalled();
  });

  it.each([
    ['a PDF', 'artifacts/report.pdf'],
    ['an image', 'artifacts/chart.png'],
    ['a video', 'artifacts/clip.mp4'],
    ['a repo diff', 'repo:3f9c1b2e-0d44-4a71-9f6d-2e5b8c7a1d03:diff#change-7:src/main.rs'],
  ])('opens %s at the top, selecting nothing', (_label, filePath) => {
    handleNavigationRequest({ target: 'file', file_path: filePath, line: 5 });

    expect(selectedLines.value).toBeNull();
    expect(lineScrollTarget.value).toBeNull();
    expect(filePreviewSource.value).toBe(false);
    expect(showToast).not.toHaveBeenCalled();
  });

  it('still selects in a repo file opened in file mode', () => {
    handleNavigationRequest({ target: 'file', file_path: 'repo:3f9c1b2e-0d44-4a71-9f6d-2e5b8c7a1d03:file:src/main.rs', line: 5 });

    expect(selectedLines.value).toEqual({ start: 5, end: 5 });
  });

  it('selects in an extensionless file, which is textual by default', () => {
    handleNavigationRequest({ target: 'file', file_path: 'repo:3f9c1b2e-0d44-4a71-9f6d-2e5b8c7a1d03:file:Makefile', line: 5 });

    expect(selectedLines.value).toEqual({ start: 5, end: 5 });
  });

  it('leaves nothing selected when the navigate has no file_path at all', () => {
    handleNavigationRequest({ target: 'file', line: 42 });

    expect(selectedLines.value).toBeNull();
    expect(lineScrollTarget.value).toBeNull();
    expect(showToast).toHaveBeenCalledWith('Navigation target missing file_path', 'error');
  });
});

describe('handleNavigationRequest: app target at a fragment', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('hands the fragment to openAppById as its third argument', () => {
    handleNavigationRequest(
      { target: 'app', app_id: 'habit-tracker', fragment: 'habit-hydration' },
      { source: 'a notification' },
    );

    expect(openAppById).toHaveBeenCalledWith(
      'habit-tracker',
      'a notification',
      'habit-hydration',
    );
    expect(showToast).not.toHaveBeenCalled();
  });

  it('passes undefined when the navigate names no place inside the app', () => {
    handleNavigationRequest({ target: 'app', app_id: 'habit-tracker' });

    expect(openAppById).toHaveBeenCalledWith('habit-tracker', undefined, undefined);
  });

  it('carries the fragment through the app-ui alias too', () => {
    handleNavigationRequest({
      target: 'app-ui',
      app_id: 'habit-tracker',
      fragment: 'habit-hydration',
    });

    expect(openAppById).toHaveBeenCalledWith('habit-tracker', undefined, 'habit-hydration');
  });

  it('still reports a navigate with no app_id, fragment or not', () => {
    handleNavigationRequest({ target: 'app', fragment: 'habit-hydration' });

    expect(openAppById).not.toHaveBeenCalled();
    expect(showToast).toHaveBeenCalledWith('Navigation target missing app_id', 'error');
  });

  it('names the fragment in the jump offer, the way the file case names a line', () => {
    expect(
      describeNavTarget({ target: 'app', app_id: 'habit-tracker', fragment: 'habit-hydration' }),
    ).toBe('app "habit-tracker#habit-hydration"');
    expect(describeNavTarget({ target: 'app', app_id: 'habit-tracker' }))
      .toBe('app "habit-tracker"');
    expect(describeNavTarget({ target: 'app', fragment: 'habit-hydration' })).toBe('an app');
  });
});

describe('handleNavigationRequest: thread target', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    panelOverlay.value = null;
  });

  it('leaves app fullscreen so the conversation it lands in is on screen', () => {
    handleNavigationRequest({ target: 'thread', id: 't-1', event_id: 'e-7' });

    expect(exitAppFullscreen).toHaveBeenCalledTimes(1);
    expect(focusThreadOrBootstrap).toHaveBeenCalledWith('t-1', { targetEventId: 'e-7' });
  });

  it('keeps the app the reader had open', () => {
    const overlay = { type: 'app-ui' as const, app: { id: 'habit-tracker', name: 'Habit Tracker' } };
    panelOverlay.value = overlay as never;

    handleNavigationRequest({ target: 'thread', id: 't-1' });

    expect(panelOverlay.value).toBe(overlay as never);
    expect(revealContentPane).not.toHaveBeenCalled();
  });

  it('reports a thread target with no id, and navigates nowhere', () => {
    handleNavigationRequest({ target: 'thread' });

    expect(showToast).toHaveBeenCalledWith('Navigation target missing thread id', 'error');
    expect(focusThreadOrBootstrap).not.toHaveBeenCalled();
    expect(exitAppFullscreen).not.toHaveBeenCalled();
  });
});

describe('handleNavigationRequest: url target', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('sends an oauth-marked url to the authorization opener, never openUrl', () => {
    handleNavigationRequest({
      target: 'url',
      url: 'https://accounts.google.com/o/oauth2/auth',
      purpose: 'oauth',
    });

    expect(openOAuthAuthorizationUrl).toHaveBeenCalledWith(
      'https://accounts.google.com/o/oauth2/auth',
    );
    expect(openUrl).not.toHaveBeenCalled();
  });

  it('sends an ordinary url to openUrl, with the source that asked', () => {
    handleNavigationRequest(
      { target: 'url', url: 'https://example.com/report' },
      { source: 'thread "Weekly report"' },
    );

    expect(openUrl).toHaveBeenCalledWith('https://example.com/report', 'thread "Weekly report"');
    expect(openOAuthAuthorizationUrl).not.toHaveBeenCalled();
  });

  it('reports a url target with no url instead of opening nothing quietly', () => {
    handleNavigationRequest({ target: 'url', purpose: 'oauth' });

    expect(showToast).toHaveBeenCalledWith('Navigation target missing url', 'error');
    expect(openOAuthAuthorizationUrl).not.toHaveBeenCalled();
    expect(openUrl).not.toHaveBeenCalled();
  });
});

describe('navigate_ui contract is fully consumable by the frontend', () => {
  const renderable = new Set<string>(SETTINGS_SUBVIEW_ITEMS.map((i) => i.key));

  it('every advertised settings_view is a renderable Settings subview', () => {
    for (const view of SETTINGS_VIEW_TARGETS) {
      expect(renderable.has(view), `settings_view "${view}" is not renderable`).toBe(true);
    }
  });

  it('every advertised navigate target is handled (no "unknown target")', () => {
    for (const target of NAVIGATE_TARGETS) {
      vi.clearAllMocks();
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
