import { describe, it, expect, afterEach, beforeEach, vi } from 'vitest';
import { normalizeDataPath } from './artifacts';

const mockListArtifacts = vi.fn();
vi.mock('../../api/client', () => ({
  listArtifacts: (...args: unknown[]) => mockListArtifacts(...args),
  uploadFile: vi.fn(),
}));

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

  it.each([
    'themes/harbour.json',
    'config/apis.json',
    'auth-modules/binance-hmac.wasm',
    'scripts/auth/login.py',
  ])('leaves %s unchanged', (path) => {
    expect(normalizeDataPath(path)).toBe(path);
  });

  it('handles bare filename without directory', () => {
    expect(normalizeDataPath('readme.md')).toBe('artifacts/readme.md');
  });

  it('still prefixes a plain path with artifacts/', () => {
    expect(normalizeDataPath('notes.md')).toBe('artifacts/notes.md');
  });

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

  it('normalizes a malformed repo: path like any other data path', () => {
    expect(normalizeDataPath('repo:r1:weird:a.md')).toBe('artifacts/repo:r1:weird:a.md');
  });
});

const PDF = 'artifacts/projects/reports/cover-letter.pdf';

describe('loadArtifacts — previously-open file preview restore', () => {
  beforeEach(() => {
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
    localStorage.setItem('file-preview-open', PDF);
    const { panelOverlay } = await import('../store');
    panelOverlay.value = null;

    const { loadArtifacts } = await import('./artifacts');
    await loadArtifacts();

    panelOverlay.value = { type: 'app-ui', app: { id: 'demo-director' } as never };

    await loadArtifacts();

    expect(panelOverlay.value).toEqual({ type: 'app-ui', app: { id: 'demo-director' } });
  });

  it('drops a stale saved path that no longer exists, on the first load', async () => {
    localStorage.setItem('file-preview-open', 'artifacts/deleted.md');
    mockListArtifacts.mockResolvedValue({ artifacts: [PDF] });
    const { panelOverlay } = await import('../store');
    panelOverlay.value = null;

    const { loadArtifacts } = await import('./artifacts');
    await loadArtifacts();

    expect(panelOverlay.value).toBeNull();
    expect(localStorage.getItem('file-preview-open')).toBeNull();
  });
});

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
    lineScrollTarget.value = { path: 'artifacts/notes.md', line: 5 };

    openFilePreview('artifacts/other.md');

    expect(selectedLines.value).toBeNull();
    expect(lineScrollTarget.value).toBeNull();
  });

  it('drops a find request no bar took, so it cannot open on a later visit', async () => {
    const { findRequest } = await import('../store');
    const { openFilePreview } = await import('./artifacts');
    findRequest.value = { scope: 'file:artifacts/notes.md', query: 'apple', line: 3 };

    openFilePreview('artifacts/other.md');

    expect(findRequest.value).toBeNull();
  });

  it('drops them on the reload restore path too', async () => {
    const { selectedLines, lineScrollTarget } = await import('../store');
    const { openFilePreview } = await import('./artifacts');
    selectedLines.value = { start: 5, end: 10 };
    lineScrollTarget.value = { path: 'artifacts/notes.md', line: 5 };

    openFilePreview('artifacts/other.md', { preserveSource: true });

    expect(selectedLines.value).toBeNull();
    expect(lineScrollTarget.value).toBeNull();
  });
});

describe('invalidateFilePreview: only the file on screen re-reads', () => {
  beforeEach(() => {
    vi.resetModules();
    localStorage.clear();
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  async function showing(path: string | null) {
    const store = await import('../store');
    store.panelOverlay.value = path ? { type: 'file-preview', path } : null;
    return { store, actions: await import('./artifacts') };
  }

  it('bumps the stamp for the file the preview shows', async () => {
    const { store, actions } = await showing('artifacts/clip.mp4');
    actions.invalidateFilePreview('artifacts/clip.mp4');
    vi.runAllTimers();
    expect(store.filePreviewRevision.value).toEqual({ path: 'artifacts/clip.mp4', rev: 1 });
  });

  it('leaves the stamp untouched for any other file', async () => {
    const { store, actions } = await showing('artifacts/clip.mp4');
    actions.invalidateFilePreview('artifacts/notes.md');
    vi.runAllTimers();
    expect(store.filePreviewRevision.value).toBeNull();
  });

  it('keeps a bumped stamp stable across a write to a different file', async () => {
    const { store, actions } = await showing('artifacts/clip.mp4');
    actions.invalidateFilePreview('artifacts/clip.mp4');
    vi.runAllTimers();
    const stamped = store.filePreviewRevision.value;

    actions.invalidateFilePreview('artifacts/notes.md');
    vi.runAllTimers();
    expect(store.filePreviewRevision.value).toEqual(stamped);
  });

  it('does nothing when the pane shows something other than a file', async () => {
    const { store, actions } = await showing(null);
    actions.invalidateFilePreview('artifacts/clip.mp4');
    vi.runAllTimers();
    expect(store.filePreviewRevision.value).toBeNull();
  });

  it('coalesces a burst of announcements into one bump', async () => {
    const { store, actions } = await showing('artifacts/clip.mp4');
    actions.invalidateFilePreview('artifacts/clip.mp4');
    actions.invalidateFilePreview('artifacts/clip.mp4');
    actions.invalidateFilePreview('artifacts/clip.mp4');
    vi.runAllTimers();
    expect(store.filePreviewRevision.value).toEqual({ path: 'artifacts/clip.mp4', rev: 1 });
  });

  it('refreshFilePreview bumps whatever the pane shows, no path asked for', async () => {
    const { store, actions } = await showing('knowhow/ops/deploy.md');
    actions.refreshFilePreview();
    vi.runAllTimers();
    expect(store.filePreviewRevision.value).toEqual({ path: 'knowhow/ops/deploy.md', rev: 1 });
  });

  it('settles a media refresh as soon as its bump is applied', async () => {
    const { actions } = await showing('artifacts/clip.mp4');
    let settled = false;
    void actions.refreshFilePreview().then(() => { settled = true; });
    vi.runAllTimers();
    await Promise.resolve();
    expect(settled).toBe(true);
  });

  it('settles a text refresh only once the text body reports its re-read', async () => {
    const { actions } = await showing('knowhow/ops/deploy.md');
    const unmount = actions.registerPreviewTextBody();
    let settled = false;
    void actions.refreshFilePreview().then(() => { settled = true; });
    vi.runAllTimers();
    await Promise.resolve();
    expect(settled).toBe(false);
    actions.reportPreviewTextSettled(0);
    await Promise.resolve();
    expect(settled).toBe(false);
    actions.reportPreviewTextSettled(1);
    await Promise.resolve();
    expect(settled).toBe(true);
    unmount();
  });

  it('keeps a refresh waiting across a later bump in the debounce window', async () => {
    const { actions } = await showing('artifacts/clip.mp4');
    let settled = false;
    void actions.refreshFilePreview().then(() => { settled = true; });
    actions.invalidateFilePreview('artifacts/clip.mp4');
    vi.runAllTimers();
    await Promise.resolve();
    expect(settled).toBe(true);
  });

  it('settles a waiting refresh when the text body unmounts', async () => {
    const { actions } = await showing('knowhow/ops/deploy.md');
    const unmount = actions.registerPreviewTextBody();
    let settled = false;
    void actions.refreshFilePreview().then(() => { settled = true; });
    vi.runAllTimers();
    unmount();
    await Promise.resolve();
    expect(settled).toBe(true);
  });

  it('answers the open repo preview through openFilePreviewRevision', async () => {
    const encoded = 'repo:r1:file:src/main.rs';
    const { store, actions } = await showing(encoded);
    expect(store.openFilePreviewRevision.value).toBe(0);

    actions.refreshFilePreview();
    vi.runAllTimers();
    expect(store.openFilePreviewRevision.value).toBe(1);
  });

  it('answers 0 once the pane moves off the file the stamp names', async () => {
    const { store, actions } = await showing('repo:r1:file:src/main.rs');
    actions.refreshFilePreview();
    vi.runAllTimers();

    store.panelOverlay.value = { type: 'file-preview', path: 'repo:r1:file:src/other.rs' };
    expect(store.openFilePreviewRevision.value).toBe(0);
  });

  it('loadArtifacts refreshes the list and leaves the stamp alone', async () => {
    mockListArtifacts.mockResolvedValue({ artifacts: ['artifacts/clip.mp4'] });
    const { store, actions } = await showing('artifacts/clip.mp4');
    await actions.loadArtifacts();
    vi.runAllTimers();
    expect(store.artifacts.value).toEqual({ status: 'loaded', data: ['artifacts/clip.mp4'] });
    expect(store.filePreviewRevision.value).toBeNull();
  });
});

describe('loadArtifacts keeps the newest answer', () => {
  beforeEach(() => {
    vi.resetModules();
    localStorage.clear();
    mockListArtifacts.mockReset();
  });

  function deferredListing() {
    let answer!: (paths: string[]) => void;
    const promise = new Promise<{ artifacts: string[] }>(resolve => {
      answer = paths => resolve({ artifacts: paths });
    });
    return { promise, answer };
  }

  it('lists a file announced during the first load', async () => {
    const first = deferredListing();
    const second = deferredListing();
    mockListArtifacts.mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise);
    const { artifacts } = await import('../store');
    const { loadArtifacts, refreshArtifacts } = await import('./artifacts');

    const startup = loadArtifacts();
    refreshArtifacts();
    first.answer(['artifacts/old.md']);
    await vi.waitFor(() => expect(mockListArtifacts).toHaveBeenCalledTimes(2));
    second.answer(['artifacts/old.md', 'artifacts/new.png']);
    await startup;

    expect(artifacts.value).toEqual({
      status: 'loaded',
      data: ['artifacts/old.md', 'artifacts/new.png'],
    });
  });

  it('never lets a listing that started earlier overwrite a later one', async () => {
    const early = deferredListing();
    const late = deferredListing();
    mockListArtifacts.mockReturnValueOnce(early.promise).mockReturnValueOnce(late.promise);
    const { artifacts } = await import('../store');
    const { loadArtifacts } = await import('./artifacts');

    const first = loadArtifacts();
    const second = loadArtifacts();
    expect(mockListArtifacts, 'one listing at a time').toHaveBeenCalledTimes(1);
    early.answer(['artifacts/a.md']);
    await vi.waitFor(() => expect(mockListArtifacts).toHaveBeenCalledTimes(2));
    late.answer(['artifacts/a.md', 'artifacts/b.md']);
    await Promise.all([first, second]);

    expect(artifacts.value).toEqual({
      status: 'loaded',
      data: ['artifacts/a.md', 'artifacts/b.md'],
    });
  });

  it('folds a burst of requests during one listing into one more', async () => {
    const first = deferredListing();
    mockListArtifacts
      .mockReturnValueOnce(first.promise)
      .mockResolvedValue({ artifacts: ['artifacts/a.md'] });
    const { loadArtifacts } = await import('./artifacts');

    const done = loadArtifacts();
    void loadArtifacts();
    void loadArtifacts();
    void loadArtifacts();
    first.answer([]);
    await done;

    expect(mockListArtifacts).toHaveBeenCalledTimes(2);
  });

  it('retries a list whose last load failed', async () => {
    mockListArtifacts.mockResolvedValue({ artifacts: ['artifacts/a.md'] });
    const { artifacts } = await import('../store');
    const { refreshArtifacts, loadArtifacts } = await import('./artifacts');
    artifacts.value = { status: 'failed', error: 'Request timed out after 10000ms' };
    refreshArtifacts();
    await loadArtifacts();
    expect(artifacts.value).toEqual({ status: 'loaded', data: ['artifacts/a.md'] });
  });

  it('keeps the loaded value when a refresh lists the same paths', async () => {
    mockListArtifacts
      .mockResolvedValueOnce({ artifacts: ['artifacts/a.md', 'artifacts/b.md'] })
      .mockResolvedValueOnce({ artifacts: ['artifacts/a.md', 'artifacts/b.md'] })
      .mockResolvedValueOnce({ artifacts: ['artifacts/a.md', 'artifacts/c.md'] });
    const { artifacts } = await import('../store');
    const { loadArtifacts } = await import('./artifacts');
    await loadArtifacts();
    const first = artifacts.value;

    await loadArtifacts();
    expect(artifacts.value).toBe(first);

    await loadArtifacts();
    expect(artifacts.value).toEqual({ status: 'loaded', data: ['artifacts/a.md', 'artifacts/c.md'] });
  });

  it('leaves a list nobody asked for unfetched', async () => {
    mockListArtifacts.mockResolvedValue({ artifacts: [] });
    const { refreshArtifacts } = await import('./artifacts');
    refreshArtifacts();
    expect(mockListArtifacts).not.toHaveBeenCalled();
  });
});
