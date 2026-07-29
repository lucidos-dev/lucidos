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
  const repo = file ? parseRepoPath(file) : null;
  if (repo) {
    const sel = selectedLines.value;
    return {
      repo_file_context: {
        repo_id: repo.repoId,
        path: repo.path,
        lines: sel ? [sel.start, sel.end] : undefined,
      },
    };
  }

  if (file) {
    return { file_context: { path: file } };
  }

  return null;
}
