import { currentApp, previewFile, selectedLines, parseRepoPath } from '../store';
import type { ChatRequestBody } from '../../api/types';

/** View-derived chat context — a snapshot of the visible panel overlay,
 *  passed explicitly into `sendMessage` instead of read from view-state by
 *  the action layer. Mirrors the three context fields on `ChatRequestBody`
 *  via `Pick` so the wire shape is the single source of truth. */
export type ChatContext = Pick<
  ChatRequestBody,
  'app_context' | 'file_context' | 'repo_file_context'
>;

/** Build a ChatContext from the visible panel overlay. Returns null when
 *  nothing visible is contextually relevant. */
export function currentChatContext(): ChatContext | null {
  const app = currentApp.value;
  if (app) {
    return { app_context: { app_id: app.id } };
  }

  const file = previewFile.value;
  const sel = selectedLines.value;
  const repo = file ? parseRepoPath(file) : null;
  if (repo) {
    return {
      repo_file_context: {
        repo_id: repo.repoId,
        path: repo.path,
        lines: sel ? [sel.start, sel.end] : undefined,
      },
    };
  }

  if (file) {
    // A workspace data file carries its line selection too: both previews show
    // the same line-numbered source view, so a range picked in one must reach
    // the message the same way it does in the other.
    return { file_context: { path: file, lines: sel ? [sel.start, sel.end] : undefined } };
  }

  return null;
}
