// @vitest-environment jsdom
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { beforeEach, afterEach, expect, it, vi } from 'vitest';
import { restoreSourceReview } from '../../store/restoreSourceReview';
import { RetainedRestoreSources } from './RetainedRestoreSources';
import { readRetainedRestoreSources, copyRetainedRestoreSource } from '../../store/restoreRuntime';
import { focusThreadOrBootstrap } from '../../store/actions/threads';

vi.mock('../../store/restoreRuntime', () => ({ readRetainedRestoreSources: vi.fn(), copyRetainedRestoreSource: vi.fn() }));
vi.mock('../../store/actions/threads', () => ({ focusThreadOrBootstrap: vi.fn() }));
vi.mock('../../store/store', async (importOriginal) => ({ ...await importOriginal<typeof import('../../store/store')>(), showToast: vi.fn() }));
vi.mock('../../hooks/usePanelRefresh', () => ({ usePanelRefresh: vi.fn() }));
let host: HTMLDivElement;
beforeEach(() => { vi.clearAllMocks(); host = document.createElement('div'); document.body.append(host); });
afterEach(() => { render(null, host); host.remove(); });

it('reviews unknown-owner originals and copies only a reference to an unsent draft', async () => {
  const rows = restoreSourceReview([{ pageId: 'page', generation: null, intent: {
    drafts: [{ threadId: 'original', composerSource: { ownerId: 'other', threadId: 'original' },
      draft: { text: '<original message>', mode: null, image_hashes: [] }, selection: { ccModel: 'previous-model' } }], pendingSelection: {},
  } }], null);
  vi.mocked(readRetainedRestoreSources).mockResolvedValue(rows);
  vi.mocked(copyRetainedRestoreSource).mockResolvedValue('review-draft');
  await act(async () => { render(<RetainedRestoreSources />, host); });
  await act(async () => {});
  expect(host.textContent).toContain('Another tab may still be editing');
  expect(copyRetainedRestoreSource).not.toHaveBeenCalled();
  await act(async () => { (host.querySelector('.list-section-toggle') as HTMLButtonElement).click(); });
  expect(host.textContent).toContain('<original message>');
  expect(host.querySelector('original')).toBeNull();
  expect(host.textContent).toContain('previous-model');
  await act(async () => { [...host.querySelectorAll('button')].find(b => b.textContent === 'Copy to review draft')!.click(); });
  expect(copyRetainedRestoreSource).toHaveBeenCalledExactlyOnceWith({ pageId: 'page', generation: null, sourceThreadId: 'original' });
  expect(focusThreadOrBootstrap).toHaveBeenCalledWith('review-draft');
});

it('shows a journal read failure instead of an empty list', async () => {
  vi.mocked(readRetainedRestoreSources).mockRejectedValue(new Error('journal unavailable'));
  await act(async () => { render(<RetainedRestoreSources />, host); });
  await act(async () => {});
  expect(host.textContent).toContain('journal unavailable');
  expect(host.textContent).not.toContain('No preserved sources');
});
