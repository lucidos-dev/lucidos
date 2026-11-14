import { ConfirmDialog } from './ConfirmDialog';
import { PromptDialog } from './PromptDialog';

/** The `showConfirm` and `showPrompt` dialogs, as one chunk off the first
 *  frame (ADR 0436). `App` mounts them once either has opened. */
export function ConfirmAndPromptDialogs() {
  return (
    <>
      <ConfirmDialog />
      <PromptDialog />
    </>
  );
}
