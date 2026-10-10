/**
 * What "Copy path" puts on the clipboard, and what it tells the user.
 *
 * Unlike the popout action, this one does not branch on Tauri vs browser: the
 * clipboard API itself is what decides, via `clipboardAbilities().copy`. Given
 * a clipboard, what varies is only whether an absolute disk path could be
 * resolved, which `previewCopyPath` already answers (see
 * `utils/previewPath.test.ts`). This file pins the wiring: the action reads
 * the live signals, forwards to `copyToClipboard` with the right text and
 * confirmation, and disappears where there is no clipboard to write to.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';

const clipboard = vi.hoisted(() => ({ copy: true }));
vi.mock('../../../utils/platform', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../utils/platform')>()),
  clipboardAbilities: () => clipboard,
}));

const copyToClipboard = vi.hoisted(() => vi.fn());
vi.mock('../../../utils/clipboard', () => ({ copyToClipboard }));

const { filePreviewCopyPathAction } = await import('../ContentHeaderActions');
const { repositories, workspacePath } = await import('../../../store/store');

const ARTIFACT = 'artifacts/reports/pr-1573.html';
const REPO_FILE = 'repo:repo-1:file:src/main.rs';

describe('filePreviewCopyPathAction', () => {
  beforeEach(() => {
    clipboard.copy = true;
    copyToClipboard.mockClear();
    workspacePath.value = '/home/user/workspaces/dev';
    repositories.value = {
      status: 'loaded',
      data: [{ id: 'repo-1', name: 'example-repo', path: '/home/user/code/example-repo' }],
    };
  });

  it('copies the absolute path under the workspace data dir', () => {
    filePreviewCopyPathAction(ARTIFACT)?.onClick?.({} as MouseEvent);

    expect(copyToClipboard).toHaveBeenCalledWith(
      '/home/user/workspaces/dev/data/artifacts/reports/pr-1573.html',
      'Path copied',
    );
  });

  it('copies the absolute path inside the repo checkout', () => {
    filePreviewCopyPathAction(REPO_FILE)?.onClick?.({} as MouseEvent);

    expect(copyToClipboard).toHaveBeenCalledWith('/home/user/code/example-repo/src/main.rs', 'Path copied');
  });

  it('falls back to the relative path, and says so, when the repo clone is not loaded', () => {
    repositories.value = { status: 'loading' };

    filePreviewCopyPathAction(REPO_FILE)?.onClick?.({} as MouseEvent);

    expect(copyToClipboard).toHaveBeenCalledWith(
      'src/main.rs',
      'Relative path copied (the full path is not known yet)',
    );
  });

  it('falls back to the relative path, and says so, when the workspace root is not loaded', () => {
    workspacePath.value = '';

    filePreviewCopyPathAction(ARTIFACT)?.onClick?.({} as MouseEvent);

    expect(copyToClipboard).toHaveBeenCalledWith(
      ARTIFACT,
      'Relative path copied (the full path is not known yet)',
    );
  });

  it('is the same control everywhere a clipboard exists, the key and label included', () => {
    const action = filePreviewCopyPathAction(ARTIFACT);
    expect(action?.key).toBe('copy-path');
    expect(action?.label).toBe('Copy path');
  });

  // Nothing here shows the path as selectable text, so with no clipboard a tap
  // could only open the "no clipboard" toast. Hidden instead, like every other
  // copy button in this codebase (AddDeviceSection's `CopyButton`, PairingGate).
  it('is null off a non-secure origin, where there is no clipboard to write to', () => {
    clipboard.copy = false;

    expect(filePreviewCopyPathAction(ARTIFACT)).toBeNull();
  });
});
