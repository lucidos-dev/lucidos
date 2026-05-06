import { fetchThreadEvents } from '../api/threads';
import { workspaceName, showToast } from '../store/store';

// 60 chars keeps "thread-<8>-<slug>.json" comfortably under the 255-byte
// filesystem name limit on Windows / macOS / Linux.
function titleSlug(title: string): string {
    const slug = (title || 'untitled')
        .toLowerCase()
        .replace(/[^a-z0-9]+/g, '-')
        .replace(/^-+|-+$/g, '')
        .slice(0, 60);
    return slug || 'untitled';
}

function shortId(threadId: string): string {
    return threadId.replace(/-/g, '').slice(0, 8);
}

function triggerDownload(blob: Blob, filename: string): void {
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = filename;
    document.body.appendChild(a);
    a.click();
    a.remove();
    // Defer revoke so Safari / iOS WebKit can start the download — they need
    // the object URL to remain live for at least one tick after click().
    setTimeout(() => URL.revokeObjectURL(url), 0);
}

/** Wraps the events with workspace + title + exported_at so a JSON file
 *  attached to a bug report carries enough context to identify the thread
 *  without the report author having to type it out. */
export async function exportThread(threadId: string, title: string): Promise<void> {
    try {
        const snapshot = await fetchThreadEvents(threadId);
        const envelope = {
            exported_at: new Date().toISOString(),
            workspace: workspaceName.value || 'unknown',
            thread_id: threadId,
            title: title || 'Untitled Thread',
            events: snapshot.events,
            current_aggregate: snapshot.currentAggregate,
        };
        const blob = new Blob([JSON.stringify(envelope, null, 2)], { type: 'application/json' });
        triggerDownload(blob, `thread-${shortId(threadId)}-${titleSlug(title)}.json`);
        showToast('Thread exported', 'success');
    } catch (err) {
        const msg = err instanceof Error ? err.message : String(err);
        showToast(`Failed to export thread: ${msg}`, 'error');
    }
}
