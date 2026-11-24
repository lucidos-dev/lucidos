import { workspaceName } from '../store/store';
import { REMOTE_GATEWAY } from './basePath';
import { copyToClipboard } from './clipboard';
import { linkedWorkspaceRef } from './threadLinkWorkspace';

function buildThreadRef(threadId: string, title: string): string {
    const ws = linkedWorkspaceRef(workspaceName.value || 'unknown', REMOTE_GATEWAY);
    const safeTitle = (title || 'Untitled Thread').replace(/[\[\]]/g, '');
    return `[${safeTitle}](thread:${ws}/${threadId})`;
}

export function copyThreadRef(threadId: string, title: string): void {
    copyToClipboard(buildThreadRef(threadId, title), 'Thread reference copied');
}

export function copyThreadTitle(title: string): void {
    copyToClipboard(title || 'Untitled Thread', 'Thread title copied');
}
