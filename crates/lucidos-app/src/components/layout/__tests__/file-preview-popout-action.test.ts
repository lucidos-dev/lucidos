/**
 * Where the previewed file's popout control SENDS the file, per platform.
 *
 * Three sinks and one absence, and none of them is visible in the rendered
 * markup: a button wired to the OS opener and a button wired to a URL are the
 * same button. So this pins the action SPEC, the way its app-popout sibling
 * does, rather than mounting a header that would also run progressive collapse.
 *
 * The decision that matters is the browser one. No page may navigate to
 * `file://`, so the disk route belongs to the packaged desktop client alone. A
 * repo file has no `/data/` URL either, so in a browser it must offer nothing at
 * all rather than a control that does nothing.
 */
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

const { filePreviewPopoutAction } = await import('../ContentHeaderActions');
const { repositories, workspacePath } = await import('../../../store/store');

const ARTIFACT = 'artifacts/reports/pr-1573.html';
const REPO_FILE = 'repo:repo-1:file:src/main.rs';

describe('filePreviewPopoutAction', () => {
  beforeEach(() => {
    // `lucidos.data.url` reads the page path, to spot an app asking for its own
    // bundled asset. The shell's path is never one, so every URL below is the
    // plain `/data/` mount.
    (globalThis as unknown as { window: { location: unknown } }).window.location =
      { pathname: '/dev/', search: '', href: 'https://localhost:5251/dev/' };
    platform.isTauri = false;
    platform.isIOSPwa = false;
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
    // `href !== undefined` is what makes both renderers emit an `<a>`, so an
    // onClick here would silently demote the control to a button.
    expect(action?.onClick).toBeUndefined();
    expect(action?.label).toBe('Open in new tab');
  });

  it('browser: nothing at all for a repo file, which has no URL to point at', () => {
    expect(filePreviewPopoutAction(REPO_FILE)).toBeNull();
  });

  it('packaged desktop client: hands the real file to the OS opener', () => {
    platform.isTauri = true;

    const action = filePreviewPopoutAction(ARTIFACT);

    // No href at all, not a null one: the renderers branch on `!== undefined`,
    // so `href: null` would still emit the anchor that does nothing here.
    expect(action?.href).toBeUndefined();
    action?.onClick?.({} as MouseEvent);
    expect(openLocalFile).toHaveBeenCalledWith('/home/user/workspaces/dev/data/artifacts/reports/pr-1573.html');
  });

  it('packaged desktop client: a repo file opens from its clone', () => {
    platform.isTauri = true;

    filePreviewPopoutAction(REPO_FILE)?.onClick?.({} as MouseEvent);

    expect(openLocalFile).toHaveBeenCalledWith('/home/user/code/example-repo/src/main.rs');
  });

  // The OS opener resolves an http URL to the same default browser, so a file
  // the workspace does not hold is still reachable there.
  it('packaged desktop client: falls back to the URL where there is no file of ours', () => {
    platform.isTauri = true;

    filePreviewPopoutAction('system-knowhow/glossary.md')?.onClick?.({} as MouseEvent);

    expect(openLocalFile).not.toHaveBeenCalled();
    // ABSOLUTE, which is the whole assertion. `lucidos.data.url` answers a
    // root-relative path, and `open /dev/api/v1/…` is a filesystem path the OS
    // cannot find. An anchor resolves it for free; the OS opener does not.
    expect(openUrlOutsideApp).toHaveBeenCalledWith(expect.stringMatching(/^https?:\/\//));
  });

  it('packaged desktop client: nothing for a repo whose clone is not loaded yet', () => {
    platform.isTauri = true;
    repositories.value = { status: 'loading' };

    expect(filePreviewPopoutAction(REPO_FILE)).toBeNull();
  });

  it('packaged desktop client: says "default app", since the OS picks the handler', () => {
    platform.isTauri = true;
    // The label is also the aria-label, the tooltip and the overflow-menu row
    // text, so a wrong one is wrong in three places at once.
    expect(filePreviewPopoutAction(ARTIFACT)?.label).toBe('Open in default app');
  });

  it('installed iOS PWA: no control at all', () => {
    platform.isIOSPwa = true;
    expect(filePreviewPopoutAction(ARTIFACT)).toBeNull();
  });

  it('keeps the addressing class on every platform it appears on', () => {
    for (const tauri of [false, true]) {
      platform.isTauri = tauri;
      // `extraClass` is how the e2e suite addresses this action wherever
      // collapse put it. `key` is what the collapse hook counts.
      expect(filePreviewPopoutAction(ARTIFACT)?.extraClass).toBe('file-open-in-tab');
      expect(filePreviewPopoutAction(ARTIFACT)?.key).toBe('open-in-tab');
    }
  });
});
