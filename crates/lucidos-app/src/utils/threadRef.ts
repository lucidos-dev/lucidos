import { workspaceName } from '../store/store';
import { copyToClipboard } from './clipboard';

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
