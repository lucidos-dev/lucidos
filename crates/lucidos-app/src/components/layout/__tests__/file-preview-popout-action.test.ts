import { describe, it, expect, beforeEach, vi } from 'vitest';

const platform = vi.hoisted(() => ({ isTauri: false, isIOSPwa: false }));
vi.mock('../../../utils/platform', () => ({
  isTauri: () => platform.isTauri,
  isIOS: () => false,
  isIOSPwa: () => platform.isIOSPwa,
}));

const openLocalFile = vi.hoisted(() => vi.fn());
const openUrlOutsideApp = vi.hoisted(() => vi.fn());
vi.mock('../../../store/actions/artifacts', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/actions/artifacts')>()),
  openLocalFile,
  openUrlOutsideApp,
}));

const pass = vi.hoisted(() => ({ current: null as string | null }));
vi.mock('../../../store/actions/frame-capability', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/actions/frame-capability')>()),
  currentArtifactPreviewCapability: () => pass.current,
}));

const { filePreviewPopoutAction } = await import('../ContentHeaderActions');
const { repositories, workspacePath } = await import('../../../store/store');

const ARTIFACT = 'artifacts/reports/pr-1573.html';
const REPO_FILE = 'repo:repo-1:file:src/main.rs';

describe('filePreviewPopoutAction', () => {
  beforeEach(() => {
    (globalThis as unknown as { window: { location: unknown } }).window.location =
      { pathname: '/dev/', search: '', href: 'https://localhost:5251/dev/' };
    platform.isTauri = false;
    platform.isIOSPwa = false;
    pass.current = null;
    openLocalFile.mockClear();
    openUrlOutsideApp.mockClear();
    workspacePath.value = '/home/user/workspaces/dev';
    repositories.value = {
      status: 'loaded',
      data: [{ id: 'repo-1', name: 'example-repo', path: '/home/user/code/example-repo' }],
    };
  });

  it('browser: a real anchor at the file\'s own /data/ URL', () => {
    const action = filePreviewPopoutAction(ARTIFACT);

    expect(action?.href).toBe(`/data/${ARTIFACT}`);
    expect(action?.onClick).toBeUndefined();
    expect(action?.label).toBe('Open in new tab');
  });

  it('browser: an HTML artifact carries the preview pass, and a plain file does not', () => {
    pass.current = 'tok';

    expect(filePreviewPopoutAction(ARTIFACT)?.href).toBe(`/~cap/tok/data/${ARTIFACT}`);
    expect(filePreviewPopoutAction('artifacts/reports/chart.png')?.href).toBe('/data/artifacts/reports/chart.png');
  });

  it('browser: nothing at all for a repo file, which has no URL to point at', () => {
    expect(filePreviewPopoutAction(REPO_FILE)).toBeNull();
  });

  it('packaged desktop client: hands the real file to the OS opener', () => {
    platform.isTauri = true;

    const action = filePreviewPopoutAction(ARTIFACT);

    expect(action?.href).toBeUndefined();
    action?.onClick?.({} as MouseEvent);
    expect(openLocalFile).toHaveBeenCalledWith('/home/user/workspaces/dev/data/artifacts/reports/pr-1573.html');
  });

  it('packaged desktop client: a repo file opens from its clone', () => {
    platform.isTauri = true;

    filePreviewPopoutAction(REPO_FILE)?.onClick?.({} as MouseEvent);

    expect(openLocalFile).toHaveBeenCalledWith('/home/user/code/example-repo/src/main.rs');
  });

  it('packaged desktop client: falls back to the URL where there is no file of ours', () => {
    platform.isTauri = true;

    filePreviewPopoutAction('system-knowhow/glossary.md')?.onClick?.({} as MouseEvent);

    expect(openLocalFile).not.toHaveBeenCalled();
    expect(openUrlOutsideApp).toHaveBeenCalledWith(expect.stringMatching(/^https?:\/\//));
  });

  it('packaged desktop client: nothing for a repo whose clone is not loaded yet', () => {
    platform.isTauri = true;
    repositories.value = { status: 'loading' };

    expect(filePreviewPopoutAction(REPO_FILE)).toBeNull();
  });

  it('packaged desktop client: says "default app", since the OS picks the handler', () => {
    platform.isTauri = true;
    expect(filePreviewPopoutAction(ARTIFACT)?.label).toBe('Open in default app');
  });

  it('installed iOS PWA: no control at all', () => {
    platform.isIOSPwa = true;
    expect(filePreviewPopoutAction(ARTIFACT)).toBeNull();
  });

  it('keeps the addressing class on every platform it appears on', () => {
    for (const tauri of [false, true]) {
      platform.isTauri = tauri;
      expect(filePreviewPopoutAction(ARTIFACT)?.extraClass).toBe('file-open-in-tab');
      expect(filePreviewPopoutAction(ARTIFACT)?.key).toBe('open-in-tab');
    }
  });
});
