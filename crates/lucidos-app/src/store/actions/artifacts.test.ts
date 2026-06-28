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
