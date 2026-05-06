import { exportThread } from '../../utils/exportThread';
import { DownloadIcon } from './icons';

export function ExportThreadButton({ threadId, title }: { threadId: string; title: string }) {
  return (
    <button
      type="button"
      class="icon-btn header-icon"
      onClick={() => { void exportThread(threadId, title); }}
      aria-label="Export thread"
      data-tooltip="Export thread (for bug reports)"
    >
      <DownloadIcon />
    </button>
  );
}
