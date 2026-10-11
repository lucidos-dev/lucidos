import { describe, it, expect, beforeEach, vi } from 'vitest';
import {
  filePreviewModal,
  filePreviewSource,
  filePreviewEditing,
  selectedLines,
  lineScrollTarget,
  panelOverlay,
  activeMenuItem,
  repoSource,
} from '../store';
import { resolveFileTarget } from './fileTarget';

// The escalation is the only thing that leaves the modal, so the router is the
// one collaborator worth spying on. Everything else runs for real: the point of
// this suite is that the modal resolves its locator through the SAME resolver
// the router uses.
const handleNavigationRequest = vi.fn();
vi.mock('./navigation-request', () => ({ handleNavigationRequest }));

// Module siblings of ./artifacts (which fileTarget imports for the real
// `normalizeDataPath`) reach for the API client and the pane/nav actions at
// import time. Same stubs as artifacts.test.ts.
vi.mock('../../api/client', () => ({ listArtifacts: vi.fn(), uploadFile: vi.fn() }));
vi.mock('./pane', () => ({ revealContentPane: vi.fn() }));
vi.mock('./navigation', () => ({ pushNavState: vi.fn() }));

const {
  openFilePreviewModal, closeFilePreviewModal, escalateFilePreviewModal,
  filePreviewRequestError, filePreviewBlockedReason,
} = await import('./filePreviewModal');

const REPO_ID = '3f9c1b2e-0d44-4a71-9f6d-2e5b8c7a1d03';

beforeEach(() => {
  vi.clearAllMocks();
  closeFilePreviewModal();
  filePreviewModal.value = null;
  filePreviewSource.value = false;
  filePreviewEditing.value = false;
  selectedLines.value = null;
  lineScrollTarget.value = null;
  panelOverlay.value = { type: 'app-ui', app: { id: 'habit-tracker', name: 'Habit Tracker' } } as never;
  activeMenuItem.value = 'apps';
  repoSource.value = 'the-repo-the-panel-is-bound-to';
});

describe('openFilePreviewModal: what it shows', () => {
  it('publishes the resolved locator', () => {
    openFilePreviewModal({ file_path: 'notes.md' });
    expect(filePreviewModal.value?.path).toBe('artifacts/notes.md');
  });

  // The security property: an app must not be able to reach a file through the
  // modal that `navigate('file', …)` would not open. Both go through the one
  // resolver, so a locator lands on the same path either way.
  it.each([
    'notes.md',
    'artifacts/research/report.md',
    'knowhow/domain/guide.md',
    `repo:${REPO_ID}:file:src/main.rs`,
    'repo::file:x',
    '../../etc/passwd',
    `repo:${REPO_ID}:file#origin/main:src/main.rs`,
    `repo:${REPO_ID}:diff#change-7:src/main.rs`,
  ])('resolves %s exactly as the navigate router does', (locator) => {
    openFilePreviewModal({ file_path: locator });
    expect(filePreviewModal.value?.path).toBe(resolveFileTarget(locator).path);
  });

  it('bumps the open id so a replacing preview is not mistaken for the same one', () => {
    openFilePreviewModal({ file_path: 'a.md' });
    const first = filePreviewModal.value!.id;
    openFilePreviewModal({ file_path: 'b.md' });
    expect(filePreviewModal.value!.id).toBeGreaterThan(first);
    expect(filePreviewModal.value!.path).toBe('artifacts/b.md');
  });

  // The whole point of the modal: the app the reader is in stays put.
  it('navigates nothing', () => {
    const overlay = panelOverlay.value;
    openFilePreviewModal({ file_path: 'artifacts/notes.md', line: 5 });
    expect(panelOverlay.value).toBe(overlay);
    expect(activeMenuItem.value).toBe('apps');
    closeFilePreviewModal();
    expect(panelOverlay.value).toBe(overlay);
    expect(activeMenuItem.value).toBe('apps');
  });

  // The navigate router binds the Files panel to the repository it is opening
  // (`openEncodedRepoFilePreview` → `switchRepoSource`). The modal must NOT:
  // rebinding the panel behind the modal is the navigation this feature exists to
  // avoid, and it would reset the panel's expanded folders and change list too.
  it('leaves the Files panel bound to its own repository', () => {
    openFilePreviewModal({ file_path: `repo:${REPO_ID}:file:src/main.rs`, line: 5 });
    expect(repoSource.value).toBe('the-repo-the-panel-is-bound-to');
    closeFilePreviewModal();
    expect(repoSource.value).toBe('the-repo-the-panel-is-bound-to');
  });
});

/** The view state the open modal provides to its preview body. */
const view = () => filePreviewModal.value!.view;

describe('openFilePreviewModal: the cited line', () => {
  it('selects the line, scrolls to it, and shows source so there is something to highlight', () => {
    openFilePreviewModal({ file_path: 'artifacts/notes.md', line: 10, line_end: 20 });
    expect(view().selectedLines.value).toEqual({ start: 10, end: 20 });
    expect(view().lineScrollTarget.value).toEqual({ path: 'artifacts/notes.md', line: 10 });
    expect(view().source.value).toBe(true);
    expect(filePreviewModal.value?.range).toEqual({ start: 10, end: 20 });
  });

  it('renders the document itself when no line is cited', () => {
    openFilePreviewModal({ file_path: 'artifacts/notes.md' });
    expect(view().selectedLines.value).toBeNull();
    expect(view().lineScrollTarget.value).toBeNull();
    expect(view().source.value).toBe(false);
  });

  // A citation's line number is the part that goes stale; it must never cost the
  // reader the file. Degradation is the resolver's (one rule for both surfaces),
  // so this pins that the modal honours the verdict rather than second-guessing it.
  it.each([
    ['a stale zero', 'artifacts/notes.md', 0],
    ['a fractional line', 'artifacts/notes.md', 1.5],
    ['a PDF, which has no source view', 'artifacts/report.pdf', 5],
  ])('opens the file anyway with %s', (_label, file_path, line) => {
    openFilePreviewModal({ file_path, line });
    expect(filePreviewModal.value).not.toBeNull();
    expect(view().selectedLines.value).toBeNull();
    expect(view().source.value).toBe(false);
  });

  // The diff view needs the panel's global repo state, which the modal must not
  // touch. It previews the file instead, and because that IS a file view the
  // citation is honoured.
  //
  // The locator reaches the state UNREWRITTEN, which is what keeps the change
  // id: the modal renders the file at the change's end state, not at HEAD.
  it('previews a diff locator as the file at its change, at its line', () => {
    const locator = `repo:${REPO_ID}:diff#change-7:src/main.rs`;
    openFilePreviewModal({ file_path: locator, line: 42 });
    expect(filePreviewModal.value?.path).toBe(locator);
    expect(view().selectedLines.value).toEqual({ start: 42, end: 42 });
  });
});

describe('the modal is read-only', () => {
  it('opens out of edit mode even while the Files panel is editing', () => {
    filePreviewEditing.value = true;
    openFilePreviewModal({ file_path: 'artifacts/notes.md' });
    expect(view().editing.value).toBe(false);
  });
});

// A file link in the transcript opens the modal over whatever the content pane
// shows, a Files preview included. The two must never share a highlight, a
// scroll target, the Source toggle or the editor.
describe('the modal keeps its view state apart from the Files panel', () => {
  const panelState = () => ({
    source: filePreviewSource.value,
    editing: filePreviewEditing.value,
    selection: selectedLines.value,
    scroll: lineScrollTarget.value,
  });

  beforeEach(() => {
    filePreviewSource.value = true;
    filePreviewEditing.value = true;
    selectedLines.value = { start: 3, end: 4 };
    lineScrollTarget.value = { path: 'artifacts/notes.md', line: 3 };
  });

  it('leaves the panel untouched across open, replace and close', () => {
    const before = panelState();
    openFilePreviewModal({ file_path: 'artifacts/a.md', line: 10 });
    expect(panelState()).toEqual(before);
    openFilePreviewModal({ file_path: 'artifacts/b.md', line: 20 });
    expect(panelState()).toEqual(before);
    closeFilePreviewModal();
    expect(panelState()).toEqual(before);
  });

  it('gives each open its own state, so a replacing preview starts clean', () => {
    openFilePreviewModal({ file_path: 'artifacts/a.md', line: 10 });
    const first = view();
    openFilePreviewModal({ file_path: 'artifacts/b.md' });
    expect(view()).not.toBe(first);
    expect(view().selectedLines.value).toBeNull();
  });

  it('is a no-op when no modal is open', () => {
    const before = panelState();
    closeFilePreviewModal();
    expect(filePreviewModal.value).toBeNull();
    expect(panelState()).toEqual(before);
  });
});

// What the host replies to an app with. Only a missing locator is refused:
// everything else about a request degrades rather than fails.
describe('filePreviewRequestError', () => {
  it('accepts any non-empty locator', () => {
    expect(filePreviewRequestError({ file_path: 'notes.md' })).toBeNull();
    expect(filePreviewRequestError({ file_path: `repo:${REPO_ID}:file:src/main.rs` })).toBeNull();
  });

  it.each([
    ['missing', {}],
    ['empty', { file_path: '' }],
    ['not a string', { file_path: 42 }],
  ])('refuses a %s file_path, naming what is wrong', (_label, payload) => {
    expect(filePreviewRequestError(payload)).toContain('file_path');
  });
});

// The other half of the reply: whether the host can put ANYTHING on screen.
// A modal nobody can see, with a promise that resolved, is what made the
// fullscreen bug silent, so the host refuses instead and the app's documented
// `catch { navigate('file', at) }` fallback takes over.
describe('filePreviewBlockedReason', () => {
  it('is silent when the host can render its overlays', () => {
    expect(filePreviewBlockedReason(false)).toBeNull();
  });

  // Reachable when an app calls requestFullscreen on its own content: the host
  // document's fullscreen element is then the IFRAME, which renders no DOM
  // children, so there is nowhere to put a modal and nothing outside it paints.
  it('names the reason when a fullscreen element the host does not own is up', () => {
    expect(filePreviewBlockedReason(true)).toContain('fullscreen');
  });
});

describe('escalateFilePreviewModal', () => {
  it('closes, then hands the same file and lines to the navigate router', () => {
    openFilePreviewModal({ file_path: 'notes.md', line: 10, line_end: 20 });
    escalateFilePreviewModal();

    expect(filePreviewModal.value).toBeNull();
    expect(handleNavigationRequest).toHaveBeenCalledWith({
      target: 'file',
      file_path: 'artifacts/notes.md',
      line: 10,
      line_end: 20,
    });
  });

  it('carries no lines when the glance had none', () => {
    openFilePreviewModal({ file_path: `repo:${REPO_ID}:file:src/main.rs` });
    escalateFilePreviewModal();

    expect(handleNavigationRequest).toHaveBeenCalledWith({
      target: 'file',
      file_path: `repo:${REPO_ID}:file:src/main.rs`,
      line: undefined,
      line_end: undefined,
    });
  });

  // With the locator untouched, escalating a diff citation lands on the DIFF,
  // which is exactly what `navigate('file', <that locator>)` does. Keeping the
  // escalation identical to the router's own resolution is the property the
  // whole shared-resolver design rests on; the router then applies its own line
  // rule to it, which for a diff is to drop the range.
  it('escalates a diff locator to the diff, unrewritten', () => {
    const locator = `repo:${REPO_ID}:diff#change-7:src/main.rs`;
    openFilePreviewModal({ file_path: locator, line: 42 });
    escalateFilePreviewModal();

    expect(handleNavigationRequest).toHaveBeenCalledWith({
      target: 'file',
      file_path: locator,
      line: 42,
      line_end: 42,
    });
  });

  it('does nothing with no modal open', () => {
    escalateFilePreviewModal();
    expect(handleNavigationRequest).not.toHaveBeenCalled();
  });
});
