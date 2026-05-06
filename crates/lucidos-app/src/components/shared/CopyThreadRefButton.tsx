import { copyThreadRef } from '../../utils/threadRef';
import { CopyIcon } from './icons';

export function CopyThreadRefButton({ threadId, title, stopPropagation, extraClass }: { threadId: string; title: string; stopPropagation?: boolean; extraClass?: string }) {
  return (
    <button
      type="button"
      class={`icon-btn header-icon${extraClass ? ` ${extraClass}` : ''}`}
      onClick={(e) => {
        if (stopPropagation) e.stopPropagation();
        copyThreadRef(threadId, title);
      }}
      aria-label="Copy thread reference"
      data-tooltip="Copy thread reference"
    >
      <CopyIcon />
    </button>
  );
}
