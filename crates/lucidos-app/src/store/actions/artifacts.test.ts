import { describe, it, expect, beforeEach, vi } from 'vitest';
import { normalizeDataPath } from './artifacts';

// Mock the API client so loadArtifacts() returns a controlled artifact list.
const mockListArtifacts = vi.fn();
vi.mock('../../api/client', () => ({
  listArtifacts: (...args: unknown[]) => mockListArtifacts(...args),
  uploadFile: vi.fn(),
}));

// openFilePreview (called by the restore path) reveals the content pane and
// pushes nav state — stub both so the test exercises only the restore decision.
vi.mock('./pane', () => ({ revealContentPane: vi.fn() }));
vi.mock('./navigation', () => ({ pushNavState: vi.fn() }));

describe('normalizeDataPath', () => {
  it('prepends artifacts/ when path has no known prefix', () => {
    expect(normalizeDataPath('research/lucidos-product-market-fit.md')).toBe(
      'artifacts/research/lucidos-product-market-fit.md',
    );
  });

  it('leaves artifacts/ paths unchanged', () => {
    expect(normalizeDataPath('artifacts/research/notes.md')).toBe(
      'artifacts/research/notes.md',
    );
  });

  it('leaves knowhow/ paths unchanged', () => {
    expect(normalizeDataPath('knowhow/domain/guide.md')).toBe(
      'knowhow/domain/guide.md',
    );
  });

  it('leaves apps/ paths unchanged', () => {
    expect(normalizeDataPath('apps/myapp/knowhow/file.md')).toBe(
      'apps/myapp/knowhow/file.md',
    );
  });

  it('leaves triggers/ paths unchanged', () => {
    expect(normalizeDataPath('triggers/daily/check.md')).toBe(
      'triggers/daily/check.md',
    );
  });

  it('leaves system-knowhow/ paths unchanged', () => {
    expect(normalizeDataPath('system-knowhow/best-practices.md')).toBe(
      'system-knowhow/best-practices.md',
    );
  });

  it('handles bare filename without directory', () => {
    expect(normalizeDataPath('readme.md')).toBe('artifacts/readme.md');
  });

  it('still prefixes a plain path with artifacts/', () => {
    expect(normalizeDataPath('notes.md')).toBe('artifacts/notes.md');
  });

  // A repo-encoded preview path addresses a registered local repo clone, not
  // the workspace data tree — prefixing it would make parseRepoPath reject it
  // and ContentPane would dead-end it in the /data/* mount.
  it('leaves a repo-encoded file path unchanged', () => {
    const encoded = 'repo:3f9c1b2e-0d44-4a71-9f6d-2e5b8c7a1d03:file:src/main/resources/transforms/x.jslt';
    expect(normalizeDataPath(encoded)).toBe(encoded);
  });

  it('leaves a repo-encoded file path at the clone root unchanged', () => {
    expect(normalizeDataPath('repo:r1:file:pom.xml')).toBe('repo:r1:file:pom.xml');
  });

  it('leaves a repo-encoded diff path (with its change id) unchanged', () => {
    expect(normalizeDataPath('repo:r1:diff#change-7:src/a.rs')).toBe('repo:r1:diff#change-7:src/a.rs');
  });

  // Keyed off parseRepoPath, not a bare `repo:` prefix test: a string that only
  // LOOKS repo-encoded is an ordinary data path and still normalizes.
  it('normalizes a malformed repo: path like any other data path', () => {
    expect(normalizeDataPath('repo:r1:weird:a.md')).toBe('artifacts/repo:r1:weird:a.md');
  });
});

const PDF = 'artifacts/projects/reports/cover-letter.pdf';

describe('loadArtifacts — previously-open file preview restore', () => {
  beforeEach(() => {
    // Fresh module graph each test so the once-per-page-load restore flag
    // resets (simulates a real page reload).
    vi.resetModules();
    localStorage.clear();
    mockListArtifacts.mockReset();
    mockListArtifacts.mockResolvedValue({ artifacts: [PDF] });
  });

  it('restores the saved file preview on the first load (page-reload re-hydration)', async () => {
    localStorage.setItem('file-preview-open', PDF);
    const { panelOverlay } = await import('../store');
    panelOverlay.value = null;

    const { loadArtifacts } = await import('./artifacts');
    await loadArtifacts();

    expect(panelOverlay.value).toEqual({ type: 'file-preview', path: PDF });
  });

  it('does NOT re-open the saved file preview on a later SSE-driven refresh while an app is open', async () => {
    // Page reload: the file preview was open, so the first load re-hydrates it.
    localStorage.setItem('file-preview-open', PDF);
    const { panelOverlay } = await import('../store');
    panelOverlay.value = null;

    const { loadArtifacts } = await import('./artifacts');
    await loadArtifacts(); // first load consumes the one-shot restore

    // The user then opens the Planer app — content pane is now an app-ui.
    panelOverlay.value = { type: 'app-ui', app: { id: 'plan' } as never };

    // An agent run edits artifacts/plans/index.json → DataFileEdited → another
    // loadArtifacts(). This must NOT yank the pane back to the last PDF.
    await loadArtifacts();

    expect(panelOverlay.value).toEqual({ type: 'app-ui', app: { id: 'plan' } });
  });

  it('drops a stale saved path that no longer exists, on the first load', async () => {
    localStorage.setItem('file-preview-open', 'artifacts/deleted.md');
    mockListArtifacts.mockResolvedValue({ artifacts: [PDF] }); // saved path gone
    const { panelOverlay } = await import('../store');
    panelOverlay.value = null;

    const { loadArtifacts } = await import('./artifacts');
    await loadArtifacts();

    expect(panelOverlay.value).toBeNull();
    expect(localStorage.getItem('file-preview-open')).toBeNull();
  });
});

// Both file previews render `selectedLines`, so a range picked in one file must
// not survive into the next: it would highlight whatever rows happen to sit at
// those numbers. Same for a pending scroll that never found its file (a load
// error, a format with no source view), which would otherwise fire later on an
// unrelated file.
describe('openFilePreview clears the previous file line state', () => {
  beforeEach(async () => {
    const { panelOverlay } = await import('../store');
    panelOverlay.value = null;
    localStorage.clear();
  });

  it('drops the selection and the pending scroll', async () => {
    const { selectedLines, lineScrollTarget } = await import('../store');
    const { openFilePreview } = await import('./artifacts');
    selectedLines.value = { start: 5, end: 10 };
    lineScrollTarget.value = 5;

    openFilePreview('artifacts/other.md');

    expect(selectedLines.value).toBeNull();
    expect(lineScrollTarget.value).toBeNull();
  });

  it('drops them on the reload restore path too', async () => {
    const { selectedLines, lineScrollTarget } = await import('../store');
    const { openFilePreview } = await import('./artifacts');
    selectedLines.value = { start: 5, end: 10 };
    lineScrollTarget.value = 5;

    openFilePreview('artifacts/other.md', { preserveSource: true });

    expect(selectedLines.value).toBeNull();
    expect(lineScrollTarget.value).toBeNull();
  });
});
