// Declares that this file renders markdown: its own imports reach it only dynamically.
import '../../utils/renderMarkdown';
import { expect, it, vi } from 'vitest';
const preview = vi.hoisted(() => ({ imported: vi.fn(), component: () => null }));
vi.mock('./FilePreviewInline', () => {
  preview.imported();
  return { FilePreviewInline: preview.component };
});
it('loads the data preview at the modal gate and renders the loaded component', async () => {
  const modal = await import('./FilePreviewModal');
  expect(preview.imported).not.toHaveBeenCalled();
  expect(await modal.loadFilePreviewModal()).toBe(modal.FilePreviewModal);
  expect(preview.imported).toHaveBeenCalledOnce();
  expect(modal.filePreviewModalBody('artifacts/report.md', 'desktop').type).toBe(preview.component);
}, 20_000);
