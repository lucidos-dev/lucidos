import { currentApp, previewFile, selectedLines, parseRepoPath } from '../store';
import type { ChatRequestBody } from '../../api/types';

export type ChatContext = Pick<
  ChatRequestBody,
  'app_context' | 'file_context' | 'repo_file_context'
>;

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
    return { file_context: { path: file, lines: sel ? [sel.start, sel.end] : undefined } };
  }

  return null;
}
