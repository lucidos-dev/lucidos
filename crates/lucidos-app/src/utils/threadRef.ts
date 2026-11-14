import { workspaceName } from '../store/store';
import { REMOTE_GATEWAY } from './basePath';
import { copyToClipboard } from './clipboard';
import { linkedWorkspaceRef } from './threadLinkWorkspace';

/** Refs are always workspace-qualified so they keep meaning when pasted across
 *  workspaces. Inside a remote workspace they are gateway-qualified too.
 *
 *  The link's visible text is the thread title. The title-prompt sanitizer
 *  (crates/lucidos-engine/src/engine/chat/title.rs) strips it before the LLM
 *  sees it. So a pasted ref does not bias the new thread's generated title. */
function buildThreadRef(threadId: string, title: string): string {
    const ws = linkedWorkspaceRef(workspaceName.value || 'unknown', REMOTE_GATEWAY);
    const safeTitle = (title || 'Untitled Thread').replace(/[\[\]]/g, '');
    return `[${safeTitle}](thread:${ws}/${threadId})`;
}

/** Both copies go through `utils/clipboard`, the one guard against a missing
 *  `navigator.clipboard`. A non-secure origin exposes none, so an unguarded
 *  `writeText` threw before a promise existed. Neither arm of the `then` pair
 *  ran, and the tap did nothing and said nothing. */
export function copyThreadRef(threadId: string, title: string): void {
    copyToClipboard(buildThreadRef(threadId, title), 'Thread reference copied');
}

export function copyThreadTitle(title: string): void {
    copyToClipboard(title || 'Untitled Thread', 'Thread title copied');
}
