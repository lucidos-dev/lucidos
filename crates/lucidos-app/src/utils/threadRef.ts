import { workspaceName, showToast } from '../store/store';

/** Refs are always workspace-qualified so they keep meaning when pasted across
 *  workspaces. The link's visible text is the thread title; the title-prompt
 *  sanitizer (see crates/lucidos-engine/src/engine/chat/title.rs) strips it
 *  out before the LLM sees it, so pasting a ref into a new thread does not
 *  bias the generated title toward the referenced thread's subject. */
function buildThreadRef(threadId: string, title: string): string {
    const ws = workspaceName.value || 'unknown';
    const safeTitle = (title || 'Untitled Thread').replace(/[\[\]]/g, '');
    return `[${safeTitle}](thread:${ws}/${threadId})`;
}

export function copyThreadRef(threadId: string, title: string): void {
    const ref = buildThreadRef(threadId, title);
    navigator.clipboard.writeText(ref).then(
        () => showToast('Thread reference copied', 'success'),
        () => showToast('Failed to copy reference', 'error'),
    );
}
