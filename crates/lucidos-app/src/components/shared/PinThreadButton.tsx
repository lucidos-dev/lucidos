import { handleSaveThread, handleUnsaveThread } from '../../store/actions/threads';
import { threadMap } from '../../store/store';
import { threadFolders } from '../../store/threadFolders';
import { isFamilyRoot } from '../drawer/family-graph';
import { PinButton } from './PinButton';
import { OverflowMenu } from './OverflowMenu';
import { FolderPinItems } from './FolderPinItems';
import { PinIcon } from './icons';

export function PinThreadButton({ threadId, saved, stopPropagation, extraClass, tabIndex }: {
  threadId: string;
  saved: boolean;
  stopPropagation?: boolean;
  extraClass?: string;
  tabIndex?: number;
}) {
  const thread = threadMap.value.get(threadId);
  if (!saved && threadFolders.value.length > 0 && thread && isFamilyRoot(thread, threadMap.value)) {
    return (
      <OverflowMenu
        ariaLabel="Pin thread"
        stopPropagation={stopPropagation}
        triggerAttrs={{ 'data-tone': 'glyph', ...(tabIndex === undefined ? {} : { tabindex: String(tabIndex) }) }}
        face={{ class: `icon-btn header-icon pin-btn${extraClass ? ` ${extraClass}` : ''}`, label: 'Pin thread', children: <PinIcon /> }}
        items={({ run }) => (
          <div data-surface-step="pin-menu">
            <FolderPinItems threadId={threadId} run={run} includePinned />
          </div>
        )}
      />
    );
  }
  return (
    <PinButton
      pinned={saved}
      label={saved ? 'Remove thread from Pinned section' : 'Pin thread'}
      tooltip={saved ? 'Pinned. Click to unpin' : 'Pin thread'}
      onToggle={() => {
        if (saved) void handleUnsaveThread(threadId);
        else void handleSaveThread(threadId);
      }}
      stopPropagation={stopPropagation}
      extraClass={extraClass}
      tabIndex={tabIndex}
    />
  );
}
