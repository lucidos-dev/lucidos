import { fetchThreadEvents } from '../api/threads';
import { workspaceName, showToast } from '../store/store';
import { openLocalFile } from '../store/actions/artifacts';
import { errorDetail, isAbortError } from './errorDetail';
import { isIOS, isTauri } from './platform';
import { saveToDownloads } from './tauri';

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
    setTimeout(() => URL.revokeObjectURL(url), 0);
}

function folderLabel(dir: string): string {
    const segments = dir.split(/[/\\]/).filter(Boolean);
    return segments[segments.length - 1] ?? dir;
}

function canShareFile(file: File): boolean {
    return typeof navigator.canShare === 'function' && navigator.canShare({ files: [file] });
}

function shareFile(file: File): void {
    void navigator.share({ files: [file], title: file.name }).catch((err: unknown) => {
        if (isAbortError(err)) return;
        showToast(`Couldn't share ${file.name}: ${errorDetail(err)}`, 'error');
    });
}

export async function exportThread(threadId: string, title: string): Promise<void> {
    const filename = `thread-${shortId(threadId)}-${titleSlug(title)}.json`;
    try {
        const snapshot = await fetchThreadEvents(threadId, { includeContext: true });
        const envelope = {
            exported_at: new Date().toISOString(),
            workspace: workspaceName.value || 'unknown',
            thread_id: threadId,
            title: title || 'Untitled Thread',
            events: snapshot.events,
            current_aggregate: snapshot.currentAggregate,
        };
        const json = JSON.stringify(envelope, null, 2);
        if (isTauri()) {
            const saved = await saveToDownloads(filename, json);
            showToast(`Thread exported to ${folderLabel(saved.dir)}`, 'success', {
                action: { label: 'Open folder', onClick: () => openLocalFile(saved.dir) },
            });
            return;
        }
        const blob = new Blob([json], { type: 'application/json' });
        const file = new File([blob], filename, { type: blob.type });
        const shareable = canShareFile(file);
        const shareOnly = isIOS() && shareable;
        if (!shareOnly) triggerDownload(blob, filename);
        const message = shareOnly
            ? 'Thread export ready'
            : 'Thread exported to your downloads folder';
        showToast(message, 'success', {
            action: shareable ? { label: 'Share', onClick: () => shareFile(file) } : undefined,
        });
    } catch (err) {
        showToast(`Failed to export thread: ${errorDetail(err)}`, 'error');
    }
}
