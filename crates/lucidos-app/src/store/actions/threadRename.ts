import { renameThread, suggestTitle } from '../../api/threads';
import { errorDetail } from '../../utils/errorDetail';
import { threadDisplayTitle } from '../../utils/threadTitle';
import { removeToast, showPrompt, showToast, threadMap, TOAST_AUTO_DISMISS_MS } from '../store';
import type { ThreadState } from '../thread-events';

/** The title to send for a rename, or null when there is nothing to send: a
 *  blank value, or one equal to the current title once trimmed. */
export function normalizeRename(newValue: string, currentTitle: string): string | null {
  const trimmed = newValue.trim();
  if (!trimmed || trimmed === currentTitle) return null;
  return trimmed;
}

/** Whether a rename would show. A draft is titled by its compose text, so a
 *  rename there would change nothing on screen. */
export function canRenameThread(thread: ThreadState | undefined): thread is ThreadState {
  return !!thread && thread.meta.state !== 'composing';
}

/** The title a rename starts from, or null when the thread cannot be renamed. */
function currentTitle(threadId: string): string | null {
  const thread = threadMap.value.get(threadId);
  return canRenameThread(thread) ? threadDisplayTitle(thread) : null;
}

async function rename(threadId: string, title: string, next: string): Promise<void> {
  try {
    await renameThread(threadId, next);
  } catch (err) {
    showToast(`Could not rename thread "${title}": ${errorDetail(err)}`, 'error');
  }
}

/** Ask for a new name in a dialog prefilled with the current one. */
export async function promptRenameThread(threadId: string): Promise<void> {
  const title = currentTitle(threadId);
  if (title === null) return;
  const answer = await showPrompt('Give this thread a new name.', {
    title: 'Rename thread',
    defaultValue: title,
    okLabel: 'Rename',
  });
  const next = answer === null ? null : normalizeRename(answer, title);
  if (next !== null) await rename(threadId, title, next);
}

/** Ask the engine for a name and offer it in a toast. Nothing changes unless
 *  the user takes it. */
export async function suggestThreadName(threadId: string): Promise<void> {
  const title = currentTitle(threadId);
  if (title === null) return;
  const key = `suggest-thread-name-${threadId}`;
  showToast('Finding a better name…', 'info', { key, spinning: true });
  let suggestion: string;
  try {
    suggestion = (await suggestTitle(threadId)).trim();
  } catch (err) {
    showToast(`Could not suggest a name for "${title}": ${errorDetail(err)}`, 'error', { key });
    return;
  }
  if (!suggestion || suggestion === title) {
    showToast(`"${title}" already fits this thread.`, 'info', { key, title: 'No better name', autoDismissMs: TOAST_AUTO_DISMISS_MS });
    return;
  }
  showToast(suggestion, 'info', {
    key,
    title: 'Suggested name',
    action: {
      label: 'Use it',
      variant: 'confirm',
      onClick: () => {
        removeToast(key);
        void rename(threadId, title, suggestion);
      },
    },
  });
}
