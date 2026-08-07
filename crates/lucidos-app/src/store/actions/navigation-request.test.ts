import { describe, it, expect, beforeEach, vi } from 'vitest';
import { NAVIGATE_TARGETS, SETTINGS_VIEW_TARGETS } from '@lucidos/sdk';
import { SETTINGS_NAV_ITEMS, SETTINGS_SYSTEM_SUBPANEL_ITEMS, pluginScrollTarget, selectedLines, lineScrollTarget, filePreviewSource } from '../store';

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
// The `new-chat` branch leaves a fullscreen app panel on a split layout, so the
// contract sweep below reaches this even though it never opens an app.
const exitAppFullscreen = vi.fn(() => false);
vi.mock('./apps', () => ({ openAppById, exitAppFullscreen }));

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

// Mirrors the real predicate (`parseRepoPath(path) !== null` → handled here):
// the router's only job is to hand a repo-encoded path to this action and fall
// back to openFilePreview when it declines. Its own repo-binding behavior is
// covered in repositories-nav.test.ts.
const openEncodedRepoFilePreview = vi.fn((path: string) => path.startsWith('repo:'));
vi.mock('./repositories', () => ({ openEncodedRepoFilePreview }));

const { handleNavigationRequest, describeNavTarget } = await import('./navigation-request');

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
    // The reported bug: the Backup failure notification's "Open settings"
    // landed on Settings → System → Backup correctly, but pairing
    // switchMenuItem('settings') with openSettingsSubview pushed the Settings
    // home list as its own history entry, so Back went there instead of back to
    // the notification. openSettingsSubview lands both, so the pairing is gone.
    handleNavigationRequest({ target: 'settings', settings_view: 'backup' });
    expect(openSettingsSubview).toHaveBeenCalledWith('backup');
    expect(switchMenuItem).not.toHaveBeenCalled();
  });

  it('lands the Thread Queue subpanel in ONE nav push too', () => {
    // Same pairing, same fix: `thread-queue` is a NavigateTarget of its own that
    // the frontend reinterprets as the Settings → System subpanel.
    handleNavigationRequest({ target: 'thread-queue' });
    expect(openSettingsSubview).toHaveBeenCalledWith('thread-queue');
    expect(switchMenuItem).not.toHaveBeenCalled();
  });

  it('aliases a retired settings_view onto the category that absorbed it', () => {
    // A stored notification's deep link, or an app compiled against an older
    // SDK `SettingsViewTarget`, can still name `repositories` / `mobile-access`
    // / `network-access` / `links` / `experimental`. Those must land where the
    // setting moved, not toast "Unknown settings section".
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
    // The alias must not swallow a typo: the sender can be told, and a silent
    // land-on-home would hide a real integration bug. The toast names what the
    // CALLER sent, so they can see which value was wrong.
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

  it('routes a repo-encoded path to the repo preview, intact', () => {
    handleNavigationRequest({ target: 'file', file_path: ENCODED });
    // The encoded path is handed over verbatim — ContentPane parses it and
    // mounts RepoFilePreview instead of the /data/* preview.
    expect(openEncodedRepoFilePreview).toHaveBeenCalledWith(ENCODED);
    expect(openFilePreview).not.toHaveBeenCalled();
  });

  it('toasts and opens nothing when file_path is missing', () => {
    handleNavigationRequest({ target: 'file' });
    expect(openFilePreview).not.toHaveBeenCalled();
    expect(openEncodedRepoFilePreview).not.toHaveBeenCalled();
    expect(showToast).toHaveBeenCalledWith('Navigation target missing file_path', 'error');
  });

  // The off-focus jump offer ("Thread X wants to open …") labels the
  // destination — it must read as the file, not as the encoding.
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

// A cited line: select it, scroll to it, and show the source view so there is
// something to highlight. Set AFTER the open, because both openers clear
// `selectedLines` (so a previous file's range can't leak) and openFilePreview
// resets `filePreviewSource`.
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
    expect(lineScrollTarget.value).toBe(510);
    expect(filePreviewSource.value).toBe(true);
  });

  it('selects a range in a workspace data file', () => {
    handleNavigationRequest({ target: 'file', file_path: 'artifacts/notes.md', line: 10, line_end: 20 });

    expect(openFilePreview).toHaveBeenCalledWith('artifacts/notes.md');
    expect(selectedLines.value).toEqual({ start: 10, end: 20 });
    // The scroll lands on the first line of the range, not the last.
    expect(lineScrollTarget.value).toBe(10);
    expect(filePreviewSource.value).toBe(true);
  });

  it('is a no-op with no line: the file opens at the top, unselected', () => {
    handleNavigationRequest({ target: 'file', file_path: ENCODED });

    expect(openEncodedRepoFilePreview).toHaveBeenCalledWith(ENCODED);
    expect(selectedLines.value).toBeNull();
    expect(lineScrollTarget.value).toBeNull();
    expect(filePreviewSource.value).toBe(false);
  });

  // A citation's line number is the part that goes stale. It must never cost
  // the reader the file itself, and must never raise an error at them.
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

  // The line count isn't known until the content loads, so a past-the-end line
  // is accepted here and highlights nothing; LineNumberedCode drops the
  // selection once it can see the file is too short.
  it('accepts a line past the end of the file without complaint', () => {
    handleNavigationRequest({ target: 'file', file_path: 'artifacts/notes.md', line: 9_000_000 });

    expect(openFilePreview).toHaveBeenCalledWith('artifacts/notes.md');
    expect(selectedLines.value).toEqual({ start: 9_000_000, end: 9_000_000 });
    expect(showToast).not.toHaveBeenCalled();
  });

  // A target that can never show numbered lines must not end up with an
  // invisible selection: `currentChatContext` would attach it to the next
  // message as a range naming no code.
  it.each([
    ['a PDF', 'artifacts/report.pdf'],
    ['an image', 'artifacts/chart.png'],
    ['a video', 'artifacts/clip.mp4'],
    ['a repo diff', 'repo:repo-1:diff#change-7:src/main.rs'],
  ])('opens %s at the top, selecting nothing', (_label, filePath) => {
    handleNavigationRequest({ target: 'file', file_path: filePath, line: 5 });

    expect(selectedLines.value).toBeNull();
    expect(lineScrollTarget.value).toBeNull();
    expect(filePreviewSource.value).toBe(false);
    expect(showToast).not.toHaveBeenCalled();
  });

  it('still selects in a repo file opened in file mode', () => {
    handleNavigationRequest({ target: 'file', file_path: 'repo:repo-1:file:src/main.rs', line: 5 });

    expect(selectedLines.value).toEqual({ start: 5, end: 5 });
  });

  it('selects in an extensionless file, which is textual by default', () => {
    handleNavigationRequest({ target: 'file', file_path: 'repo:repo-1:file:Makefile', line: 5 });

    expect(selectedLines.value).toEqual({ start: 5, end: 5 });
  });

  it('leaves nothing selected when the navigate has no file_path at all', () => {
    handleNavigationRequest({ target: 'file', line: 42 });

    expect(selectedLines.value).toBeNull();
    expect(lineScrollTarget.value).toBeNull();
    expect(showToast).toHaveBeenCalledWith('Navigation target missing file_path', 'error');
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
